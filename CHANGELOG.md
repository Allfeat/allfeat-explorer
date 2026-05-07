# Changelog

## [0.3.2](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.3.1...backend-v0.3.2) (2026-05-07)


### Bug Fixes

* **api:** render non-account 32-byte event fields as hex, not SS58 ([675eabb](https://github.com/Allfeat/allfeat-explorer/commit/675eabb9d87628f0c1b3a8690bc2a710401573a0))
* **api:** use MEL as Melodie native token symbol ([43a1bf7](https://github.com/Allfeat/allfeat-explorer/commit/43a1bf752875255b222ec12886946db9fd80cfa6)), closes [#8](https://github.com/Allfeat/allfeat-explorer/issues/8)

## [0.3.1](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.3.0...backend-v0.3.1) (2026-05-06)


### Bug Fixes

* **api:** resolve ATS detail by chain ats_id, not newest-first offset ([9cc2b57](https://github.com/Allfeat/allfeat-explorer/commit/9cc2b5799cebf10dc31d9bddf14536e280317a73))

## [0.3.0](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.2.1...backend-v0.3.0) (2026-04-27)


### Features

* **api:** per-runtime dispatch across mappers, projections, and provider ([d27f393](https://github.com/Allfeat/allfeat-explorer/commit/d27f3935b627c73b65ca76e26a0a8afc9b48aeaf))
* **api:** scaffold Melodie sibling runtime codegen ([308c92d](https://github.com/Allfeat/allfeat-explorer/commit/308c92de35b9f2641b65d7e53efa0754ef70ceca))


### Refactoring

* **api:** per-network metadata bundles and RpcClient runtime tagging ([8fb60a1](https://github.com/Allfeat/allfeat-explorer/commit/8fb60a149400a4d4b7b1211631b00fc5f0d90cae))

## [0.2.1](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.2.0...backend-v0.2.1) (2026-04-25)


### Performance

* **api:** batch reconcile_account_balances writes ([80c883f](https://github.com/Allfeat/allfeat-explorer/commit/80c883f765f492f086973bb0b0ea02afb64747be))
* **api:** enable gzip + brotli response compression ([263ae3e](https://github.com/Allfeat/allfeat-explorer/commit/263ae3e9a1a2a2054cd74f3436e0894f6767761d))
* **api:** share live-stream encoding across WS sessions ([19af2aa](https://github.com/Allfeat/allfeat-explorer/commit/19af2aa6651461f30e3ab4ac57458e24bf29b432))

## [0.2.0](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.1.2...backend-v0.2.0) (2026-04-24)


### Features

* **footer:** swap slogan for real build stamps + header logo ([b4e9080](https://github.com/Allfeat/allfeat-explorer/commit/b4e908059989e1ba20a4a09f34d3425aa1ca4a3d))


### Bug Fixes

* **blocks:** resolve author via session.validators instead of aura session keys ([9d30055](https://github.com/Allfeat/allfeat-explorer/commit/9d300555ebafabb90cc76778d9195ce924ebca81))
* **ci:** avoid matrix.* in job-level if (use dynamic matrix) ([ebc834f](https://github.com/Allfeat/allfeat-explorer/commit/ebc834fdde87657221c20c04dd55c834717734c0))
* **network:** correct hardcoded RPC endpoints for Allfeat and Melodie ([a6fbdf9](https://github.com/Allfeat/allfeat-explorer/commit/a6fbdf98ccf1117586b71ec88d3d254ead8dc87f))
* **rpc:** accept plaintext ws:// endpoints via from_insecure_url ([92c62bd](https://github.com/Allfeat/allfeat-explorer/commit/92c62bd5bb1889ef60252656173134e1ffac3310))
* **rpc:** warm up finalized-head supervisor at boot to unblock readyz ([719e396](https://github.com/Allfeat/allfeat-explorer/commit/719e3963754bd8f6b7c80e07c17b33a0874e846c))


### Performance

* **cache:** cut navigation latency with HTTP cache + Nitro SWR ([f7d1690](https://github.com/Allfeat/allfeat-explorer/commit/f7d16901d0cab00a21a73851fbef227946822351))


### Refactoring

* **server:** drop per-IP rate limiter now that API is cluster-internal ([cb65a17](https://github.com/Allfeat/allfeat-explorer/commit/cb65a17d1359b6885c138ec3ef7b88e3e6aaa43b))


### Documentation

* **claude:** document CI pipeline and commit conventions ([783a150](https://github.com/Allfeat/allfeat-explorer/commit/783a150b75cfdc2e403562e4b9f092f36be76944))
