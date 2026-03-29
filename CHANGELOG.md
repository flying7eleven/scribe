# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.0.0-beta3] - 2026-03-29

### Added
- Add TUI scaffolding with tab navigation, event loop, and format extraction ([a6f49c1])
- Add Sessions tab with table rendering, navigation, and drill-down ([fa81112])
- Add Events tab with inline detail and structured/JSON toggle ([0aaf3e8])
- Add Stats tab with scrollable dashboard mirroring scribe stats ([fb9eb5b])
- Add Live tab with SQLite polling, ring buffer feed, and auto-scroll ([f137fb6])
- Add filter bar with real-time client-side filtering for Sessions and Events ([dbd4989])
- Add TUI event detail enhancement with event-type-specific field display ([5774dad])
- Add migration for 10 event detail tables with FK cascade and indexes ([8f31c3b])
- Add Tier 1 detail table ingestion for tool, stop, and session events ([be5baef])
- Add Tier 2+3 detail table ingestion with SubagentStop dual-insert ([0404cc7])
- Add scribe backfill command for populating detail tables from raw_payload ([949f9c1])
- Refactor error_summary to JOIN detail tables and add model/failure query helpers ([308e1e0])
- Add max_session_duration config to cap stale sessions ([59f0084])

### Changed
- Reorder stats output — activity before directories ([7c7efb5])
- Update the README.md ([1a8fa0c])

### Removed
- Remove verbose tool failures section from scribe stats CLI output ([ee6bf92])

### Fixed
- Enable PRAGMA foreign_keys for ON DELETE CASCADE support ([e228022])

[a6f49c1]: https://github.com/flying7eleven/scribe/commit/a6f49c19c7952089c7e0b5e000b8fa5b6074da04
[fa81112]: https://github.com/flying7eleven/scribe/commit/fa81112703e0d114e80e5c9f43c6d58f55784130
[0aaf3e8]: https://github.com/flying7eleven/scribe/commit/0aaf3e805ac983b8d02556cc8003afd1e6467cf0
[fb9eb5b]: https://github.com/flying7eleven/scribe/commit/fb9eb5b5f73e951dd8f0f36e423e0fc9b78a0904
[f137fb6]: https://github.com/flying7eleven/scribe/commit/f137fb6391e85eddd41b87249498332f82454072
[dbd4989]: https://github.com/flying7eleven/scribe/commit/dbd4989c60385c5f8b4cefede7c1ee019fd730e3
[5774dad]: https://github.com/flying7eleven/scribe/commit/57749dad07428b3b256f22a618a80146acb30204
[8f31c3b]: https://github.com/flying7eleven/scribe/commit/8f31c3b5002963a82d62ddd148cbf00819c902c4
[be5baef]: https://github.com/flying7eleven/scribe/commit/be5baef4838f3f370d7ee7697d853465783f87a3
[0404cc7]: https://github.com/flying7eleven/scribe/commit/0404cc71e8ee1a9b8d9b76dad409e14f50dd89b6
[949f9c1]: https://github.com/flying7eleven/scribe/commit/949f9c1e4065b9c585a05d18d35d5e742afa638f
[308e1e0]: https://github.com/flying7eleven/scribe/commit/308e1e09321018c0affcf397db7b0b110f95e8df
[59f0084]: https://github.com/flying7eleven/scribe/commit/59f00845a1f1376a3498ad8a9b907feed11ca2af
[7c7efb5]: https://github.com/flying7eleven/scribe/commit/7c7efb53f99cac574b5f1e68cd503c63a859e65b
[1a8fa0c]: https://github.com/flying7eleven/scribe/commit/1a8fa0c8b7df477e4d9b4879c0949b8fde34ec4f
[ee6bf92]: https://github.com/flying7eleven/scribe/commit/ee6bf92f8f1415b2b3f4865c97e8deb4165f363f
[e228022]: https://github.com/flying7eleven/scribe/commit/e228022e77ef7c3db5dc69168023d5274d6c1598

## [1.0.0-beta2] - 2026-03-26

### Added
- Add self-healing config migration with comment preservation ([904ab5c])
- Add config template and auto-creation on first interactive run ([424fde6])
- Add --json output mode, integration tests, and complete E006 epic ([22bc3d6])
- Extend stats handler with dashboard sections and --since flag ([d6ec13f])
- Add formatting helpers: histogram bars, path truncation, duration ([f6c5d37])
- Add CWD index migration and extended stats DB queries ([75550d2])

### Changed
- Add MIT license ([e1d2b75])

[904ab5c]: https://github.com/flying7eleven/scribe/commit/904ab5ce8f0d9ab0eacb8c1fd5ecf537be17ff97
[424fde6]: https://github.com/flying7eleven/scribe/commit/424fde62ee6891f094572094765807649f36439d
[22bc3d6]: https://github.com/flying7eleven/scribe/commit/22bc3d6cca8e90a02f8e696a9c22166ce9801b77
[d6ec13f]: https://github.com/flying7eleven/scribe/commit/d6ec13f7b0e62660fdedb1803e601933443b1101
[f6c5d37]: https://github.com/flying7eleven/scribe/commit/f6c5d37e2c4417dadc12dbfe0136418b743cca37
[75550d2]: https://github.com/flying7eleven/scribe/commit/75550d2fa05939f1b2d1028f284ccfe287edf1f2
[e1d2b75]: https://github.com/flying7eleven/scribe/commit/e1d2b75cbc5a2bb6ce32c4659c8121d0690b7ff2

## [1.0.0-beta1] - 2026-03-24

### Added
- Scaffold Cargo project with dependencies ([296f2c7])
- Add clap CLI skeleton with all subcommand stubs ([d11dad3])
- Add SQLite connection layer with WAL mode and path resolution ([47ba961])
- Add initial migration with events and sessions tables ([55f00bc])
- Add HookInput struct skeleton in models.rs ([ac09d8a])
- Add insert_event and query_events with session upsert ([b0aca05])
- Expand HookInput with all 21 event type fields ([267a18a])
- Implement log handler with stdin parsing and DB insert ([ddbe550])
- Add hook event registry and JSON config generation ([2fc82dd])
- Implement init handler with output modes and settings merge ([60ee9cb])
- Add time parsing, dynamic query filters, and session queries ([a67d3e8])
- Add query handler with table, JSON Lines, and CSV output ([4fae85f])
- Wire query subcommand with filters and integration tests ([9e57b97])
- Implement retain subcommand with event deletion and orphan cleanup ([bfcd2a2])
- Implement stats subcommand with DB metrics and formatting ([21d2bdc])
- Add config file loading with 4-layer precedence chain ([a8878ab])
- Add auto-retention with metadata table and check-interval logic ([540637f])
- Add shell completions for bash, zsh, fish, elvish, powershell ([f94c3b7])
- Add CI workflow files for regular changes and releases ([3e4f2f3])

### Changed
- Add comprehensive README with usage, config, and architecture ([1fbcb57])
- Add init integration tests and skip DB for init ([2a644ae])
- Add E2E integration tests for all 21 event types ([773babe])

[296f2c7]: https://github.com/flying7eleven/scribe/commit/296f2c7c59306d982cc01daea22774bac6bd9798
[d11dad3]: https://github.com/flying7eleven/scribe/commit/d11dad3b109c20f994a49ead41d8a0c71c51ec13
[47ba961]: https://github.com/flying7eleven/scribe/commit/47ba9614e12fbc4a7a49ba0daf222f96eee441f3
[55f00bc]: https://github.com/flying7eleven/scribe/commit/55f00bcd74fc4435b68131ac9e60dc90f5f5c40f
[ac09d8a]: https://github.com/flying7eleven/scribe/commit/ac09d8a976dbb249452eb2c2fd9a6b8e7bc952ee
[b0aca05]: https://github.com/flying7eleven/scribe/commit/b0aca0557472592adc13b3683f1d0d7de1222b4d
[267a18a]: https://github.com/flying7eleven/scribe/commit/267a18a89ecb0602b55ca92c6c5dae7b6a697995
[ddbe550]: https://github.com/flying7eleven/scribe/commit/ddbe55056221fdedc144891141384aa3e480d17f
[2fc82dd]: https://github.com/flying7eleven/scribe/commit/2fc82ddbaf690b62035f1b5b76ef4502dd7ba1e0
[60ee9cb]: https://github.com/flying7eleven/scribe/commit/60ee9cbe6b07d66bfeeaa0bcfac3cb5e1f7504b4
[a67d3e8]: https://github.com/flying7eleven/scribe/commit/a67d3e8cbe08938f03a0565c26e2b218c7236fa7
[4fae85f]: https://github.com/flying7eleven/scribe/commit/4fae85fb56a9fafe4ecc6c7fc4ac3ff0849a82cf
[9e57b97]: https://github.com/flying7eleven/scribe/commit/9e57b97bba5c38194f14f3cfe4812b9a9d41373e
[bfcd2a2]: https://github.com/flying7eleven/scribe/commit/bfcd2a2c415958061670f0de1d289d06794247eb
[21d2bdc]: https://github.com/flying7eleven/scribe/commit/21d2bdcee66678cb59fc5d4b9373e2454437331c
[a8878ab]: https://github.com/flying7eleven/scribe/commit/a8878abfa40c3d3d2773f2dc603e49bb543b5722
[540637f]: https://github.com/flying7eleven/scribe/commit/540637f72b3680dda54a85c6f57ed065891b534f
[f94c3b7]: https://github.com/flying7eleven/scribe/commit/f94c3b738711c66860b83235e1096f7039deae0d
[3e4f2f3]: https://github.com/flying7eleven/scribe/commit/3e4f2f3d2ac51a5e4d4328481b9a04daa25f601e
[1fbcb57]: https://github.com/flying7eleven/scribe/commit/1fbcb57245edbcedc62b00c7ad17d26728044707
[2a644ae]: https://github.com/flying7eleven/scribe/commit/2a644aecfe969e9590302eba5637a1d4dfbbdb21
[773babe]: https://github.com/flying7eleven/scribe/commit/773babe50781d42acf129d865c0490c4f720874b
