# tool broker

ava routes every model-requested tool execution through a broker before the
tool implementation runs. the broker owns effect classification, policy review,
and the final call into the executor.

the first implementation is deliberately small and runs in the ava process. it
uses an `approve_all` policy while preserving the existing user approval gate.
this creates an architectural boundary for development and testing, but it is
not a security boundary: code running in the ava process still has the same OS
authority as the broker.

## invariants

- the agent calls the broker, never the raw tool executor.
- the broker derives effects from the raw tool call. it does not trust an
  agent-provided risk label or summary.
- unknown tools receive the `unknown` effect classification.
- arbitrary command execution, policy changes, harness extensions, and harness
  modification have explicit effect classifications.
- review and execution happen in one component so later policies can bind a
  decision to the exact object that is executed.

## intended isolation boundary

the broker interface is expected to move to IPC once its request and result
protocol is stable. the agent can then run without ambient process, network,
secret, or host filesystem authority. a broker outside that isolation boundary
will review requests, obtain temporary or persistent user approval, execute the
approved effect, and return only the result.

until that isolation exists, the broker log is useful for building an effect
inventory and spotting requests that attempt to alter ava's policy, skills, or
harness. it must not be described as containment.

future restrictive policies should deny unknown effects by default and treat
changes to the broker, harness, installation, and policy store as the highest
risk class. those changes should move toward checkpointed, user-installed
releases rather than unrestricted self-modification.
