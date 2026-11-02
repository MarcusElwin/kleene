"""Harbor agent adapter for kleene.

`Harbor <https://github.com/laude-institute/harbor>`_ is the runner behind
Terminal-Bench 2.0: every task is a Docker image, an instruction and a test
script, and an "installed agent" is a CLI that Harbor installs inside the
task's container and runs once with the instruction. This module makes
``kleene`` such an agent, so a Terminal-Bench run is::

    export ANTHROPIC_API_KEY=sk-ant-...
    PYTHONPATH=. harbor run -d terminal-bench@2.0 \\
        -a adapters.kleene_harbor:KleeneAgent -m anthropic/claude-haiku-4-5 \\
        --ak budget_dollars=1 -n 4

The adapter installs the released binary with ``install.sh`` (or uploads a
local build with ``--ak binary=target/release/kleene``), writes a router
TOML that points every alias at the ``-m`` model, runs ``kleene run`` over
the instruction in the task's working directory, and reads token counts and
dollars back out of the trace tables with ``kleene trace --json``. Nothing
in the task's loop changes: the model writes CallSQL, ``shell``,
``write_file`` and ``patch`` do the work, and ``FINAL`` ends the session.

The file depends on ``harbor`` (``uv tool install harbor``) and nothing else.
"""

from __future__ import annotations

import json
import shlex
from pathlib import Path, PurePosixPath
from typing import Any

from pydantic import Field

from harbor.agents.agent_auth_resolution import ModelConnectionSpec, ProviderAccess
from harbor.agents.capabilities import AgentCapabilities
from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.agents.options import InstalledAgentOptions
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.model_api_format import ModelApiFormat

INSTALL_SCRIPT_URL = (
    "https://raw.githubusercontent.com/MarcusElwin/kleene/main/install.sh"
)

#: Appended to every instruction unless a prompt template is given. The task
#: text says what to do; this says how a kleene session ends.
TASK_SUFFIX = """\

You are working on a Linux machine. The task above is to be carried out in the \
directory {workspace}, which is your workspace; shell('...') runs commands there \
and may change directory to any path the task names. Do the work with shell, \
write_file and patch, check it the way the task would be checked (run the \
program, read the file back, run the tests), and fix what fails. The task is \
graded by what is left on the machine, not by your answer: when the work is \
done and verified, end with FINAL over one row with column done = true and a \
column note saying what you did. Do not stop early, do not ask questions, and \
do not end with FINAL before the task is complete."""

#: Query that totals the run's model calls. Memo hits cost nothing and are
#: counted separately so a replayed run shows as such.
USAGE_SQL = """\
SELECT count(*) AS calls,
       coalesce(sum(CASE WHEN memo_hit THEN 1 ELSE 0 END), 0) AS memo_hits,
       coalesce(sum(input_tokens), 0) AS input_tokens,
       coalesce(sum(cache_read_tokens), 0) AS cache_read_tokens,
       coalesce(sum(output_tokens), 0) AS output_tokens,
       coalesce(sum(cost_usd), 0.0) AS cost_usd
FROM trace_calls"""

#: Kleene's provider names for Harbor's.
PROVIDERS = {"anthropic": "anthropic", "openai": "openai_compat"}

#: Dollars per million tokens (input, output, cache read, cache write) for the
#: models kleene's own rate cards know (`kleene-llm/src/router.rs`). A router
#: TOML replaces those cards, so the adapter writes the price back in; a
#: dollar budget needs one, and the cost Harbor records comes from it. Any
#: other model takes ``--ak price=in/out[/cache_read[/cache_write]]``.
PRICES: dict[str, tuple[float, float, float, float]] = {
    "claude-opus-5-5": (4.0, 20.0, 0.4, 5.0),
    "claude-opus-5": (5.0, 25.0, 0.5, 6.25),
    "claude-sonnet-5": (2.0, 10.0, 0.2, 2.5),
    "claude-haiku-4-5": (1.0, 5.0, 0.1, 1.25),
    "gpt-6-luna": (0.10, 0.50, 0.01, 0.125),
}


def parse_price(text: str) -> tuple[float, float, float, float]:
    """``in/out[/cache_read[/cache_write]]`` in dollars per million tokens."""
    parts = [float(p) for p in text.split("/")]
    if len(parts) < 2 or len(parts) > 4 or any(p < 0 for p in parts):
        raise ValueError(
            f"price must be in/out[/cache_read[/cache_write]] dollars per million "
            f"tokens, got {text!r}"
        )
    inp, out = parts[0], parts[1]
    cache_read = parts[2] if len(parts) > 2 else inp * 0.1
    cache_write = parts[3] if len(parts) > 3 else inp * 1.25
    return inp, out, cache_read, cache_write


class KleeneOptions(InstalledAgentOptions):
    """``--ak key=value`` options the adapter accepts."""

    max_turns: int = Field(
        default=60, description="Turn cap for the root session (kleene run --max-turns)."
    )
    max_depth: int = Field(
        default=2, description="Deepest child session allowed (kleene run --max-depth)."
    )
    budget_dollars: float | None = Field(
        default=None, description="Dollar budget for the whole run, children included."
    )
    budget_calls: int | None = Field(
        default=None, description="Model-call budget for the whole run."
    )
    workspace: str = Field(
        default="/app",
        description="Directory the tools are confined to and shell runs in.",
    )
    binary: str | None = Field(
        default=None,
        description="Local kleene binary to upload instead of installing a release.",
    )
    check: str | None = Field(
        default=None,
        description="Shell command that must exit 0 before FINAL is accepted.",
    )
    log: str = Field(default="info", description="kleene --log filter for kleene.txt.")
    price: str | None = Field(
        default=None,
        description="Model price as in/out[/cache_read[/cache_write]] dollars per "
        "million tokens, for models kleene's rate cards do not know.",
    )


class KleeneAgent(BaseInstalledAgent):
    """Run ``kleene`` on a Harbor task."""

    options_model = KleeneOptions
    options: KleeneOptions
    capabilities = AgentCapabilities(base_url=True, reasoning_effort=True)

    MODEL_CONNECTION = ModelConnectionSpec(
        default_provider="anthropic",
        # No agent-level key names: the provider in the model name decides
        # (anthropic/... reads ANTHROPIC_API_KEY, openai/... OPENAI_API_KEY),
        # so a machine with both keys never sends one to the other's endpoint.
        agent_provider_access=ProviderAccess(),
        api_key_destination_names={
            "anthropic": "ANTHROPIC_API_KEY",
            "openai": "OPENAI_API_KEY",
            "default": "ANTHROPIC_API_KEY",
        },
        base_url_destination_names={
            "anthropic": "ANTHROPIC_BASE_URL",
            "openai": "OPENAI_BASE_URL",
            "default": "ANTHROPIC_BASE_URL",
        },
        supported_api_formats=[
            ModelApiFormat.ANTHROPIC_MESSAGES,
            ModelApiFormat.OPENAI_CHAT_COMPLETIONS,
        ],
        default_api_format_by_provider={
            "anthropic": ModelApiFormat.ANTHROPIC_MESSAGES,
            "openai": ModelApiFormat.OPENAI_CHAT_COMPLETIONS,
            "default": ModelApiFormat.ANTHROPIC_MESSAGES,
        },
    )

    @staticmethod
    def name() -> str:
        return "kleene"

    def get_version_command(self) -> str | None:
        return "kleene --version"

    def parse_version(self, stdout: str) -> str:
        # `kleene --version` prints "kleene 0.5.0".
        return stdout.strip().split()[-1]

    # -- paths in the container -------------------------------------------

    @property
    def _db_path(self) -> PurePosixPath:
        return self.environment_logs_dir / "kleene.duckdb"

    @property
    def _usage_path(self) -> PurePosixPath:
        return self.environment_logs_dir / "usage.json"

    @property
    def _router_path(self) -> PurePosixPath:
        return self.environment_logs_dir / "router.toml"

    @property
    def _instruction_path(self) -> PurePosixPath:
        return self.environment_logs_dir / "instruction.txt"

    # -- install ----------------------------------------------------------

    async def install(self, environment: BaseEnvironment) -> None:
        """Put ``kleene`` on the PATH: a local binary if given, else the release."""
        if self.options.binary:
            source = Path(self.options.binary)
            if not source.is_file():
                raise FileNotFoundError(f"kleene binary not found: {source}")
            await environment.upload_file(source, "/usr/local/bin/kleene")
            await self.exec_as_root(
                environment, "chmod 755 /usr/local/bin/kleene && kleene --version"
            )
            return
        await self.ensure_system_dependencies(environment, ("curl", "tar"))
        env: dict[str, str] = {"KLEENE_INSTALL": "/usr/local/bin"}
        if self.options.version:
            env["KLEENE_VERSION"] = self.options.version
        await self.exec_as_root(
            environment,
            f"curl -fsSL {INSTALL_SCRIPT_URL} | sh && kleene --version",
            env=env,
        )

    # -- run --------------------------------------------------------------

    def _provider_and_model(self) -> tuple[str, str]:
        if not self.model_name:
            raise ValueError(
                "kleene needs a model: -m anthropic/<model> or -m openai/<model>"
            )
        provider, model = self._parse_model_name(self.model_name)
        provider = provider or "anthropic"
        if provider not in PROVIDERS:
            raise ValueError(
                f"kleene speaks the anthropic and openai wire formats; got provider "
                f"'{provider}' in {self.model_name!r}"
            )
        return provider, model

    def router_toml(self) -> str:
        """A router file that resolves every alias to the ``-m`` model.

        One model everywhere keeps the comparison with other agents on the
        leaderboard fair: a run advertised as Haiku is all Haiku, including
        the proxies and child sessions.
        """
        provider, model = self._provider_and_model()
        candidate = (
            f'candidates = [{{ provider = "{PROVIDERS[provider]}", model = "{model}" }}]'
        )
        options = ["cache_prefix = true"]
        if self.options.reasoning_effort:
            options.append(f'effort = "{self.options.reasoning_effort}"')
        lines = []
        for alias in ("root", "worker", "proxy", "judge"):
            lines.append(f"[aliases.{alias}]")
            lines.append(candidate)
            lines.append("options = { " + ", ".join(options) + " }")
            lines.append("")
        price = parse_price(self.options.price) if self.options.price else PRICES.get(model)
        if price is None and self.options.budget_dollars is not None:
            raise ValueError(
                f"budget_dollars needs a price for {model!r}, which kleene's rate "
                "cards do not know: add --ak price=in/out[/cache_read[/cache_write]] "
                "(dollars per million tokens) or budget by calls (--ak budget_calls=N)"
            )
        if price is not None:
            inp, out, cache_read, cache_write = price
            lines += [
                f'[pricing."{model}"]',
                f"input_per_mtok = {inp}",
                f"output_per_mtok = {out}",
                f"cache_read_per_mtok = {cache_read}",
                f"cache_write_per_mtok = {cache_write}",
                "",
            ]
        return "\n".join(lines)

    def run_command(self, instruction_path: str) -> str:
        """The shell command that runs the task and leaves usage.json behind."""
        o = self.options
        logs = self.environment_logs_dir.as_posix()
        run = [
            "kleene",
            "--workspace",
            shlex.quote(o.workspace),
            "--db",
            shlex.quote(self._db_path.as_posix()),
            "--log",
            shlex.quote(o.log),
            "run",
            f"@{instruction_path}",
            "--max-turns",
            str(o.max_turns),
            "--max-depth",
            str(o.max_depth),
            "--json",
        ]
        if o.budget_dollars is not None:
            run += ["--budget-dollars", str(o.budget_dollars)]
        if o.budget_calls is not None:
            run += ["--budget-calls", str(o.budget_calls)]
        if o.check:
            run += ["--check", shlex.quote(o.check)]
        usage = [
            "kleene",
            "--db",
            shlex.quote(self._db_path.as_posix()),
            "trace",
            "--json",
            shlex.quote(USAGE_SQL),
        ]
        # `kleene run` exits 3 when the session ended without FINAL (turns or
        # budget exhausted). That is a result for the verifier to grade, not
        # an agent crash, so the exit code is recorded and the command exits 0
        # unless kleene itself failed (no key, no binary, bad flags).
        return (
            f"mkdir -p {shlex.quote(logs)} && cd {shlex.quote(o.workspace)} && "
            f"{{ {' '.join(run)} > {shlex.quote(logs + '/result.json')} "
            f"2> {shlex.quote(logs + '/kleene.txt')}; rc=$?; }}; "
            f"echo \"$rc\" > {shlex.quote(logs + '/exit_code')}; "
            f"{' '.join(usage)} > {shlex.quote(logs + '/usage.json')} 2>/dev/null || true; "
            "[ \"$rc\" -eq 0 ] || [ \"$rc\" -eq 3 ]"
        )

    def _write_text_command(self, path: PurePosixPath, text: str) -> str:
        marker = "KLEENE_EOF_7c1e9a"
        if marker in text:
            raise ValueError("instruction contains the heredoc marker")
        return (
            f"mkdir -p {shlex.quote(path.parent.as_posix())} && "
            f"cat > {shlex.quote(path.as_posix())} << '{marker}'\n{text}\n{marker}\n"
        )

    @with_prompt_template
    async def run(
        self, instruction: str, environment: BaseEnvironment, context: AgentContext
    ) -> None:
        if self._prompt_template_path is None:
            instruction = instruction + TASK_SUFFIX.format(
                workspace=self.options.workspace
            )
        access = self.model_connection
        if not access.api_key:
            raise ValueError(
                "kleene needs an API key: set ANTHROPIC_API_KEY (anthropic/...) or "
                "OPENAI_API_KEY (openai/...) in the environment Harbor runs in"
            )
        env = {
            **access.env,
            "KLEENE_ROUTER_TOML": self._router_path.as_posix(),
            # Never pick up a config.toml or MCP servers from the image.
            "KLEENE_CONFIG_DIR": (self.environment_logs_dir / "config").as_posix(),
            "NO_COLOR": "1",
        }
        await self.exec_as_agent(
            environment,
            self._write_text_command(self._router_path, self.router_toml())
            + self._write_text_command(self._instruction_path, instruction),
        )
        await self.exec_as_agent(
            environment,
            self.run_command(self._instruction_path.as_posix()),
            env=env,
            cwd=self.options.workspace,
        )

    # -- metrics ----------------------------------------------------------

    @staticmethod
    def parse_usage(text: str) -> dict[str, Any] | None:
        """The one row ``kleene trace --json`` printed for :data:`USAGE_SQL`."""
        try:
            rows = json.loads(text)
        except json.JSONDecodeError:
            return None
        if not isinstance(rows, list) or not rows or not isinstance(rows[0], dict):
            return None
        return rows[0]

    def populate_context_post_run(self, context: AgentContext) -> None:
        usage_file = self.logs_dir / "usage.json"
        if not usage_file.is_file():
            return
        row = self.parse_usage(usage_file.read_text())
        if row is None:
            return
        context.n_input_tokens = int(row.get("input_tokens") or 0)
        context.n_cache_tokens = int(row.get("cache_read_tokens") or 0)
        context.n_output_tokens = int(row.get("output_tokens") or 0)
        context.cost_usd = float(row.get("cost_usd") or 0.0)
        metadata: dict[str, Any] = {
            "calls": int(row.get("calls") or 0),
            "memo_hits": int(row.get("memo_hits") or 0),
        }
        exit_code = self.logs_dir / "exit_code"
        if exit_code.is_file():
            metadata["kleene_exit_code"] = exit_code.read_text().strip()
        result = self.logs_dir / "result.json"
        if result.is_file():
            try:
                metadata["final"] = json.loads(result.read_text())
            except json.JSONDecodeError:
                pass
        context.metadata = metadata
