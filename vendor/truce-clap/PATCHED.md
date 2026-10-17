Unmodified copy of truce-clap 6.3.0 from crates.io except for two CLAP
spec-conformance fixes in src/lib.rs (search "Magnitude patch"):

1. `clap_host_latency::changed` is no longer called outside `activate`
   (an active plugin only calls `request_restart`).
2. `state.load` and preset loading request `params.rescan(CLAP_PARAM_RESCAN_VALUES)`.

Both were found with clap-validator 0.4.1. Licensed under the Truce
License 1.0 (MIT / Apache-2.0 for plug-ins); see LICENSE.

Proposed upstream: https://github.com/truce-audio/truce/pull/238
Delete this directory and the [patch.crates-io] entry once a truce-clap
release includes it.
