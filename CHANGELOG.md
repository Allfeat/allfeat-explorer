# Changelog

## [0.2.0](https://github.com/Allfeat/allfeat-explorer/compare/backend-v0.1.2...backend-v0.2.0) (2026-04-24)


### Features

* **footer:** swap slogan for real build stamps + header logo ([b4e9080](https://github.com/Allfeat/allfeat-explorer/commit/b4e908059989e1ba20a4a09f34d3425aa1ca4a3d))


### Bug Fixes

* **blocks:** resolve author via session.validators instead of aura session keys ([9d30055](https://github.com/Allfeat/allfeat-explorer/commit/9d300555ebafabb90cc76778d9195ce924ebca81))
* **network:** correct hardcoded RPC endpoints for Allfeat and Melodie ([a6fbdf9](https://github.com/Allfeat/allfeat-explorer/commit/a6fbdf98ccf1117586b71ec88d3d254ead8dc87f))
* **rpc:** accept plaintext ws:// endpoints via from_insecure_url ([92c62bd](https://github.com/Allfeat/allfeat-explorer/commit/92c62bd5bb1889ef60252656173134e1ffac3310))
* **rpc:** warm up finalized-head supervisor at boot to unblock readyz ([719e396](https://github.com/Allfeat/allfeat-explorer/commit/719e3963754bd8f6b7c80e07c17b33a0874e846c))


### Performance

* **cache:** cut navigation latency with HTTP cache + Nitro SWR ([f7d1690](https://github.com/Allfeat/allfeat-explorer/commit/f7d16901d0cab00a21a73851fbef227946822351))


### Refactoring

* **server:** drop per-IP rate limiter now that API is cluster-internal ([cb65a17](https://github.com/Allfeat/allfeat-explorer/commit/cb65a17d1359b6885c138ec3ef7b88e3e6aaa43b))


### Documentation

* **claude:** document CI pipeline and commit conventions ([783a150](https://github.com/Allfeat/allfeat-explorer/commit/783a150b75cfdc2e403562e4b9f092f36be76944))
