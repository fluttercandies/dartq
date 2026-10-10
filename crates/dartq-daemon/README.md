# dartq-daemon

The dartq daemon: an LSP shell over the multi-context analysis runtime.
One client on stdin/stdout, a reader thread feeding a message channel, and
the request handlers that drive analysis contexts, snapshots, and plugin
sessions.

## Role in the workspace

Wraps the command cores in `dartq-commands` (one implementation per
command) with transport and session state, using `dartq-snapshot` for
persistent state and `dartq-plugin` for plugin sessions; `dartq-cli`
launches it as the `dartq daemon` face.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
