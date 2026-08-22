# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.25](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.24...v0.0.25) - 2026-08-22

### Added

- [**breaking**] say which distribution an install came from ([#50](https://github.com/wavekat/wavekat-platform-client/pull/50))

## [0.0.24](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.23...v0.0.24) - 2026-08-08

### Added

- *(voice)* booking calls + schema negotiation ([#48](https://github.com/wavekat/wavekat-platform-client/pull/48))

## [0.0.23](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.22...v0.0.23) - 2026-07-31

### Added

- *(voice)* add deleted_at to VoiceCallRecord ([#46](https://github.com/wavekat/wavekat-platform-client/pull/46))

## [0.0.22](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.21...v0.0.22) - 2026-07-26

### Added

- *(voice)* add call-flow step traces to call records ([#44](https://github.com/wavekat/wavekat-platform-client/pull/44))

## [0.0.21](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.20...v0.0.21) - 2026-07-26

### Added

- *(voice)* add call-flow attribution to call records ([#41](https://github.com/wavekat/wavekat-platform-client/pull/41))

## [0.0.20](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.19...v0.0.20) - 2026-07-18

### Added

- *(voice)* flow version asset endpoints ([#39](https://github.com/wavekat/wavekat-platform-client/pull/39))

## [0.0.19](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.18...v0.0.19) - 2026-07-13

### Added

- *(voice)* published call-flows pull endpoint (doc 48) ([#37](https://github.com/wavekat/wavekat-platform-client/pull/37))

## [0.0.18](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.17...v0.0.18) - 2026-07-03

### Added

- *(voice)* add negotiated codec to call records ([#35](https://github.com/wavekat/wavekat-platform-client/pull/35))

## [0.0.17](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.16...v0.0.17) - 2026-06-28

### Added

- *(voice)* add transferred_local end reason + transfer_target ([#33](https://github.com/wavekat/wavekat-platform-client/pull/33))

## [0.0.16](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.15...v0.0.16) - 2026-06-26

### Added

- *(voice)* add per-channel default-mute to share ([#31](https://github.com/wavekat/wavekat-platform-client/pull/31))

## [0.0.15](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.14...v0.0.15) - 2026-06-24

### Added

- *(voice)* add allow_download to share request/state ([#29](https://github.com/wavekat/wavekat-platform-client/pull/29))

## [0.0.14](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.13...v0.0.14) - 2026-06-24

### Fixed

- re-export PartyMasking from the crate root ([#27](https://github.com/wavekat/wavekat-platform-client/pull/27))

## [0.0.13](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.12...v0.0.13) - 2026-06-24

### Added

- *(voice)* recording share visibility ([#25](https://github.com/wavekat/wavekat-platform-client/pull/25))

## [0.0.12](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.11...v0.0.12) - 2026-06-21

### Added

- read a recording's share state back ([#23](https://github.com/wavekat/wavekat-platform-client/pull/23))

## [0.0.11](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.10...v0.0.11) - 2026-06-21

### Added

- add recording share command pair to Client ([#19](https://github.com/wavekat/wavekat-platform-client/pull/19))

## [0.0.10](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.9...v0.0.10) - 2026-06-20

### Added

- add VoiceAccounts sync resource ([#20](https://github.com/wavekat/wavekat-platform-client/pull/20))

## [0.0.9](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.8...v0.0.9) - 2026-06-07

### Added

- add ConnectionLost voice-call end reason ([#18](https://github.com/wavekat/wavekat-platform-client/pull/18))

### Other

- link wavekat.com from README ([#16](https://github.com/wavekat/wavekat-platform-client/pull/16))

## [0.0.8](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.7...v0.0.8) - 2026-06-01

### Added

- add release-keys binary for CI credential issuance ([#14](https://github.com/wavekat/wavekat-platform-client/pull/14))

## [0.0.7](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.6...v0.0.7) - 2026-06-01

### Added

- add anonymous install heartbeat method ([#11](https://github.com/wavekat/wavekat-platform-client/pull/11))

### Fixed

- Me.id is a UUID string, not an integer ([#13](https://github.com/wavekat/wavekat-platform-client/pull/13))

## [0.0.5](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.4...v0.0.5) - 2026-05-19

### Added

- add VoiceRecordings + VoiceTranscripts sync markers ([#8](https://github.com/wavekat/wavekat-platform-client/pull/8))

## [0.0.4](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.3...v0.0.4) - 2026-05-16

### Added

- add SyncEndpoint trait + VoiceCalls marker ([#6](https://github.com/wavekat/wavekat-platform-client/pull/6))

## [0.0.3](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.2...v0.0.3) - 2026-05-16

### Added

- add Error::Unauthorized variant for typed 401s ([#4](https://github.com/wavekat/wavekat-platform-client/pull/4))

## [0.0.2](https://github.com/wavekat/wavekat-platform-client/compare/v0.0.1...v0.0.2) - 2026-05-14

### Added

- port Client and loopback OAuth from wavekat-cli ([#2](https://github.com/wavekat/wavekat-platform-client/pull/2))
