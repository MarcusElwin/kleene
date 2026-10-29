# Changelog

## [0.5.0](https://github.com/MarcusElwin/kleene/compare/v0.4.0...v0.5.0) (2026-10-29)


### Features

* **algebra:** price a child session as an agent loop ([2f1362e](https://github.com/MarcusElwin/kleene/commit/2f1362e630b4df85c11a8f9b005ab204d0a4bb58))
* **cli:** --json and --out for kleene run, context diagnostics ([5b9083b](https://github.com/MarcusElwin/kleene/commit/5b9083bc9671723e09883533a61b8839b54e2a43))
* **cli:** --load name=file.csv for kleene run and kleene repl ([3aa9196](https://github.com/MarcusElwin/kleene/commit/3aa9196d2cb5c3079905bebc7a51eb59663b8692))
* **cli:** kleene dialect prints the reference that matches the binary ([fd7accd](https://github.com/MarcusElwin/kleene/commit/fd7accd08d9820931c003d89b7a510ddc0e1e5ec))
* **core:** chr, ascii, strpos, left, right, repeat, reverse, lpad, rpad ([ce24cb7](https://github.com/MarcusElwin/kleene/commit/ce24cb79f1c654b29d3f2e20dcbc1ae5e0d326c1))
* **core:** render a batch as JSON or CSV ([93329a0](https://github.com/MarcusElwin/kleene/commit/93329a01ce32a22b3f6dbb1fb5a4c8f06be13f92))
* **exec:** evaluate the new string builtins by character ([9fa6e7f](https://github.com/MarcusElwin/kleene/commit/9fa6e7f29e0deabd2b145be8a1a75047711ddd9f))
* **harness:** refuse unpriced dollar budgets and FINAL that delegates ([7e7d68d](https://github.com/MarcusElwin/kleene/commit/7e7d68d7cab5ab17dfd9fcd5fdb98355870081fd))
* **harness:** tables preloaded before a run's first turn ([597b4e7](https://github.com/MarcusElwin/kleene/commit/597b4e785ee6031c0d5b050e278693d7e72f98be))
* **llm:** Provider::cost_known, KLEENE_PRICING and gateway cost reports ([5d5619b](https://github.com/MarcusElwin/kleene/commit/5d5619b70d96dda07a2f33791f0c78964dcc9f66))
* **sql:** CREATE TABLE with a column list, FINAL SELECT ([297e255](https://github.com/MarcusElwin/kleene/commit/297e2556640826bf38dfbb22112eeb2ecf73ac1a))
* **tools:** read_csv and read_json table functions ([b63d305](https://github.com/MarcusElwin/kleene/commit/b63d305c457b3e89d2744ec9ba36da65b05215a3))


### Bug Fixes

* **harness:** keep the SQL after a multi-byte string literal ([f905da3](https://github.com/MarcusElwin/kleene/commit/f905da3f946a2424337d1fbcf9dd9549c866391b))

## [0.4.0](https://github.com/MarcusElwin/kleene/compare/v0.3.1...v0.4.0) (2026-10-25)


### Features

* **daemon:** named-pipe transport so the daemon builds on Windows ([9497b1c](https://github.com/MarcusElwin/kleene/commit/9497b1cb2f917e4b5468c7bbb302e3d059d2a142)), closes [#59](https://github.com/MarcusElwin/kleene/issues/59)

## [0.3.1](https://github.com/MarcusElwin/kleene/compare/v0.3.0...v0.3.1) (2026-10-11)


### Bug Fixes

* **harness:** drop the previous task's plan table when a new run starts ([b5c765c](https://github.com/MarcusElwin/kleene/commit/b5c765cc49ac7bd58a4393b1e481746104b755ce))
* **sql:** let CALL arguments and scripts carry dollar-quoted bodies ([3649080](https://github.com/MarcusElwin/kleene/commit/3649080f7710aa5fad349be263f5f759bb6673b6))

## [0.3.0](https://github.com/MarcusElwin/kleene/compare/v0.2.0...v0.3.0) (2026-10-11)


### Features

* **core:** add ConversationId for runs that continue one another ([1331308](https://github.com/MarcusElwin/kleene/commit/1331308344e25e06a64ade551e600a1b719cbdfb))
* **harness:** remember conversations in SQL tables ([14bd0e0](https://github.com/MarcusElwin/kleene/commit/14bd0e0ff1701dc19d3e68552fc2c146be4785d9))
* remember conversations in SQL tables ([2796aff](https://github.com/MarcusElwin/kleene/commit/2796affca021b80ccb0dfff195c3c91547a0f63c))
* **tui:** show the kleene version and warn when the daemon is older ([a5458e0](https://github.com/MarcusElwin/kleene/commit/a5458e09aa71bc921f023e24bdab5b81b64e59fd))
* **tui:** show the kleene version, and warn when the daemon is an older one ([5785ff1](https://github.com/MarcusElwin/kleene/commit/5785ff1ce5bfd8babe8f169b22349c5ea17e8fa0))
* **tui:** spinner from Enter until the model's first call ([c34c0e6](https://github.com/MarcusElwin/kleene/commit/c34c0e629941429dbcd65e92084d3c0dea9277ae))
* **tui:** spinner from Enter until the model's first call ([2e20c59](https://github.com/MarcusElwin/kleene/commit/2e20c590f3ba6327109f7a2f4d4098bd299248fc))
