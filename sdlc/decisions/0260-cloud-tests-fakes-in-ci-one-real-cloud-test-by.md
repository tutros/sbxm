260. **Cloud tests: fakes in CI, one real cloud test by hand (2026-10-23, the user's answer; builds on [248]):** CI
uses `FakeBackend`; the cloud command lines and parsers are tested on S14's captured output. An ignored
`tests/real_sbx_cloud.rs`, run by hand with an environment variable, walks one tiny task through the cloud on the
small shape: egress accepting all three refusal shapes (S14 Q1), a fake-secret leak check, the TTL set, the bundle
in and out, and removal. It runs at the milestone's end-to-end check. No cloud credentials go into GitHub CI.
