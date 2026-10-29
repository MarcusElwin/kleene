"""Tests for the Harbor adapter, without Docker or a model.

Run with ``python -m pytest adapters`` using an interpreter that has
``harbor`` (``uv run --with harbor --with pytest -m pytest adapters``).
A fake environment records the commands the adapter would run in the
container and plays back the files kleene would leave behind.
"""

from __future__ import annotations

import json
import shlex
from dataclasses import dataclass, field
from pathlib import Path

import pytest

from adapters.kleene_harbor import TASK_SUFFIX, USAGE_SQL, KleeneAgent, parse_price
from harbor.models.agent.context import AgentContext


@dataclass
class Exec:
    command: str
    user: str | int | None
    env: dict[str, str] | None
    cwd: str | None


@dataclass
class Result:
    return_code: int = 0
    stdout: str = ""
    stderr: str = ""


@dataclass
class FakeEnvironment:
    """Just enough of ``BaseEnvironment`` for the adapter: exec and upload."""

    execs: list[Exec] = field(default_factory=list)
    uploads: list[tuple[Path, str]] = field(default_factory=list)
    fail_matching: str | None = None

    async def exec(self, command, cwd=None, env=None, timeout_sec=None, user=None):
        self.execs.append(Exec(command, user, env, cwd))
        if self.fail_matching and self.fail_matching in command:
            return Result(return_code=1, stderr="boom")
        return Result()

    async def upload_file(self, source_path, target_path):
        self.uploads.append((Path(source_path), target_path))


@pytest.fixture
def key(monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-ant-test")
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)


def agent(tmp_path: Path, **kwargs) -> KleeneAgent:
    kwargs.setdefault("model_name", "anthropic/claude-haiku-4-5")
    return KleeneAgent(logs_dir=tmp_path, **kwargs)


def test_router_points_every_alias_at_the_model(tmp_path, key):
    toml = agent(tmp_path, reasoning_effort="low").router_toml()
    for alias in ("root", "worker", "proxy", "judge"):
        assert f"[aliases.{alias}]" in toml
    assert toml.count('{ provider = "anthropic", model = "claude-haiku-4-5" }') == 4
    assert 'effort = "low"' in toml and "cache_prefix = true" in toml


def test_openai_models_route_through_openai_compat(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "sk-test")
    toml = agent(tmp_path, model_name="openai/gpt-5.4-mini").router_toml()
    assert 'provider = "openai_compat", model = "gpt-5.4-mini"' in toml


def test_unknown_provider_is_refused(tmp_path, monkeypatch):
    monkeypatch.setenv("GEMINI_API_KEY", "x")
    with pytest.raises(ValueError, match="anthropic and openai"):
        agent(tmp_path, model_name="gemini/gemini-3-pro").router_toml()


def test_run_command_passes_the_options_and_tolerates_no_final(tmp_path, key):
    a = agent(
        tmp_path,
        max_turns=40,
        max_depth=1,
        budget_dollars=2.5,
        budget_calls=200,
        check="make test",
        workspace="/work dir",
    )
    cmd = a.run_command("/logs/agent/instruction.txt")
    assert "cd '/work dir'" in cmd
    assert "--workspace '/work dir'" in cmd
    assert "run @/logs/agent/instruction.txt --max-turns 40 --max-depth 1 --json" in cmd
    assert "--budget-dollars 2.5 --budget-calls 200 --check 'make test'" in cmd
    assert "trace --json " + shlex.quote(USAGE_SQL) in cmd
    assert cmd.endswith('[ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]')
    for name in ("result.json", "kleene.txt", "exit_code", "usage.json"):
        assert f"/logs/agent/{name}" in cmd


def test_default_options(tmp_path, key):
    cmd = agent(tmp_path).run_command("/logs/agent/instruction.txt")
    assert "--max-turns 60 --max-depth 2 --json" in cmd
    assert "--budget" not in cmd and "--check" not in cmd
    assert cmd.startswith("mkdir -p /logs/agent && cd /app && ")


@pytest.mark.asyncio
async def test_install_runs_the_release_installer_as_root(tmp_path, key):
    env = FakeEnvironment()
    await agent(tmp_path, version="v0.5.0").install(env)
    install = [e for e in env.execs if "install.sh" in e.command]
    assert len(install) == 1
    assert install[0].user == "root"
    assert install[0].env == {"KLEENE_INSTALL": "/usr/local/bin", "KLEENE_VERSION": "v0.5.0"}
    assert "kleene --version" in install[0].command


@pytest.mark.asyncio
async def test_install_uploads_a_local_binary_instead(tmp_path, key):
    binary = tmp_path / "kleene"
    binary.write_bytes(b"ELF")
    env = FakeEnvironment()
    await agent(tmp_path, binary=str(binary)).install(env)
    assert env.uploads == [(binary, "/usr/local/bin/kleene")]
    assert not any("install.sh" in e.command for e in env.execs)
    assert any("chmod 755 /usr/local/bin/kleene" in e.command for e in env.execs)


@pytest.mark.asyncio
async def test_install_refuses_a_missing_binary(tmp_path, key):
    with pytest.raises(FileNotFoundError):
        await agent(tmp_path, binary=str(tmp_path / "nope")).install(FakeEnvironment())


@pytest.mark.asyncio
async def test_run_writes_router_and_instruction_then_runs_kleene(tmp_path, key):
    a = agent(tmp_path, budget_dollars=1)
    env = FakeEnvironment()
    await a.run("Make /app/out.txt say hi.", env, AgentContext())
    assert len(env.execs) == 2
    setup, run = env.execs
    assert "cat > /logs/agent/router.toml" in setup.command
    assert "cat > /logs/agent/instruction.txt" in setup.command
    assert "Make /app/out.txt say hi." in setup.command
    assert TASK_SUFFIX.format(workspace="/app") in setup.command
    assert run.cwd == "/app"
    assert run.env["ANTHROPIC_API_KEY"] == "sk-ant-test"
    assert run.env["KLEENE_ROUTER_TOML"] == "/logs/agent/router.toml"
    assert run.env["KLEENE_CONFIG_DIR"] == "/logs/agent/config"
    assert "kleene --workspace /app --db /logs/agent/kleene.duckdb" in run.command


@pytest.mark.asyncio
async def test_run_without_a_key_fails_before_touching_the_container(tmp_path, monkeypatch):
    monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
    monkeypatch.delenv("ANTHROPIC_AUTH_TOKEN", raising=False)
    env = FakeEnvironment()
    with pytest.raises(Exception):
        await agent(tmp_path).run("x", env, AgentContext())
    assert not any("kleene --workspace" in e.command for e in env.execs)


@pytest.mark.asyncio
async def test_a_failing_kleene_raises(tmp_path, key):
    env = FakeEnvironment(fail_matching="kleene --workspace")
    with pytest.raises(RuntimeError):
        await agent(tmp_path).run("x", env, AgentContext())


def test_usage_fills_the_context(tmp_path, key):
    (tmp_path / "usage.json").write_text(
        json.dumps(
            [
                {
                    "calls": 12,
                    "memo_hits": 2,
                    "input_tokens": 34000,
                    "cache_read_tokens": 30000,
                    "output_tokens": 2100,
                    "cost_usd": 0.0421,
                }
            ]
        )
    )
    (tmp_path / "exit_code").write_text("0\n")
    (tmp_path / "result.json").write_text('[{"done": true, "note": "wrote out.txt"}]')
    ctx = AgentContext()
    agent(tmp_path).populate_context_post_run(ctx)
    assert ctx.n_input_tokens == 34000
    assert ctx.n_cache_tokens == 30000
    assert ctx.n_output_tokens == 2100
    assert ctx.cost_usd == pytest.approx(0.0421)
    assert ctx.metadata == {
        "calls": 12,
        "memo_hits": 2,
        "kleene_exit_code": "0",
        "final": [{"done": True, "note": "wrote out.txt"}],
    }


def test_missing_or_broken_usage_leaves_the_context_empty(tmp_path, key):
    ctx = AgentContext()
    agent(tmp_path).populate_context_post_run(ctx)
    assert ctx.is_empty()
    (tmp_path / "usage.json").write_text("Error: no such table")
    agent(tmp_path).populate_context_post_run(ctx)
    assert ctx.is_empty()


def test_parse_version():
    assert KleeneAgent.parse_version(None, "kleene 0.5.0\n") == "0.5.0"


def test_the_provider_in_the_model_name_picks_the_key(tmp_path, monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-ant-test")
    monkeypatch.setenv("OPENAI_API_KEY", "sk-oa-test")
    anthropic = agent(tmp_path).model_connection.env
    openai = agent(tmp_path, model_name="openai/gpt-5.4-mini").model_connection.env
    assert anthropic["ANTHROPIC_API_KEY"] == "sk-ant-test" and "OPENAI_API_KEY" not in anthropic
    assert openai["OPENAI_API_KEY"] == "sk-oa-test" and "ANTHROPIC_API_KEY" not in openai


def test_known_models_are_priced_in_the_router(tmp_path, key):
    toml = agent(tmp_path).router_toml()
    assert '[pricing."claude-haiku-4-5"]' in toml
    assert "input_per_mtok = 1.0" in toml and "output_per_mtok = 5.0" in toml
    assert "cache_read_per_mtok = 0.1" in toml and "cache_write_per_mtok = 1.25" in toml


def test_an_unknown_model_with_a_dollar_budget_needs_a_price(tmp_path, key):
    with pytest.raises(ValueError, match="price"):
        agent(tmp_path, model_name="anthropic/claude-next", budget_dollars=1).router_toml()
    toml = agent(tmp_path, model_name="anthropic/claude-next", budget_dollars=1, price="3/15").router_toml()
    assert '[pricing."claude-next"]' in toml
    assert "input_per_mtok = 3.0" in toml and "cache_write_per_mtok = 3.75" in toml
    # Without a dollar budget an unpriced model runs; its cost reads zero.
    assert "[pricing" not in agent(tmp_path, model_name="anthropic/claude-next").router_toml()


def test_parse_price():
    assert parse_price("1/5") == (1.0, 5.0, 0.1, 1.25)
    assert parse_price("2/10/0.5/3") == (2.0, 10.0, 0.5, 3.0)
    for bad in ("1", "1/2/3/4/5", "a/b", "-1/2"):
        with pytest.raises(ValueError):
            parse_price(bad)
