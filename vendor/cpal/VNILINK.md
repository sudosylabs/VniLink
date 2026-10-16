# Local CPAL 0.18.2 patch

Source: crates.io cpal 0.18.2, Apache-2.0 (LICENSE). Original crate SHA-256: `6f02e8d0327b42d3e2e4ab2119af397344eb9fc54a34bf0ddeaa1277af8681f1`.

PulseAudio latency workers previously waited synchronously for a server timing
reply during Drop. A silent/paused stream could hold native cleanup for the
request timeout. The patch makes each timing future abortable, registers the
abort under the same lock used by cancellation, and aborts before joining.
Record streams also queue their native delete before joining, matching playback.
No callback or other OS backend is changed.

A metadata-only `DeviceTrait::is_loopback_source` query forwards the PulseAudio
server monitor-of-sink field through the platform enum. VniLink uses it to keep
approved system monitors separate from approved microphones; names are not
used to guess source permissions. Other backends retain the default false.

The explicit null-sink native regression in vnilink-audio measures stream Drop
and rejects cleanup longer than two seconds. Physical Linux/PipeWire server
loss and permissions must still be validated. Remove this patch when an
upstream release provides equivalent cancellation. This vendor package has an
isolated workspace to avoid running upstream device-opening examples/tests.

The PulseAudio host exposes `default_loopback_device`, resolving the monitor of the current default sink by the server-provided sink index. VniLink uses it for the System default audio source; enumeration neither activates capture nor requests consent.
