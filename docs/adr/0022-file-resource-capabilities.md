# ADR-0022: Permission-Typed File Resource Capabilities

- Status: Accepted for implementation trial.
- Date: November 9, 2026
- Amends: ADR-0008, ADR-0013, and ADR-0019.

## Problem

Journal declares append and read as independent Effect capabilities. A shared
relative filename cannot guarantee that the host binds both Drivers to the same
root. The requirement should express authority over one logical file, with the
operations available on that file visible in its type.

## Decision

Under the `fs` feature, `File<P>` is a sealed requirement family. Its issued
`FileCapability<P>` carries one Program-issued resource identity. The initial
sealed permission sets are `ReadOnly`, `AppendOnly`, `ReadAppend`, `WriteOnly`,
and `ReadWrite`. Write authority permits whole-file replacement and append;
append authority alone does not authorize replacement or truncation. None of
these sets authorizes removal. The permission type
controls which operation constructors exist. A capability may be cloned or
narrowed to read-only, append-only, write-only, or read-and-append, when those
operations are permitted, without changing its resource identity.
Applications cannot construct or widen capabilities themselves.

One `FileDriver::new(path)` binds a File requirement position. It supplies all
permitted operations against that single target and performs no I/O until a
runtime-owned effect task executes. Separate File positions may have identical
permission types but bind different targets; neither names nor marker types
are needed to distinguish them. A File denotes a logical pathname, not a
persistent OS handle or an inode protected against external replacement.

`file.read()` produces `FileEffect<FileRead>` and `file.append_line(text)` (or
`append_bytes` / `append_text`) produces `FileEffect<FileAppend>`. These values
are inert. `into_command()` uses Message conversion, `into_command_with(mapper)`
maps a typed outcome, and `discard_outcome()` issues the operation without an
application continuation. FileRead returns Bytes; FileAppend returns unit;
both use FsError. Operation descriptors contain no caller-selectable target.

`file.replace_bytes(contents)` and `file.replace_text(contents)` require
`CanWrite` and produce `FileEffect<FileReplace>`, returning unit or FsError.
The Driver reuses `ReplaceFile`'s temporary-sibling write, file sync, and rename
mechanism: a reader sees the old or new contents, with no partial target write.
Parent directories are created lazily. The directory entry is not synced;
replacement does not promise durability across a crash. There is no positional
write or open-handle API in this slice.

Live effect binding gains exact capability selection. An exact requirement
must have exactly one matching binding for each allowed operation; a type-wide
binding cannot satisfy it. Duplicate, foreign, wrong-operation, or overlapping
type-wide/exact bindings fail before execution. Existing ordinary Effect<D>
requirements retain their type-wide behavior. Runtime declaration validation
checks the pair (capability, descriptor type), allowing one File identity to
authorize several separately typed operations.

`control_all()` selects each declared file operation by exact identity and
never instantiates FileDriver. `next_file_read(&file)` and
`next_file_append(&file)` / `next_file_replace(&file)` select pending typed effects for that resource, even
when another file has an earlier operation of the same type. Foreign selectors
fail without taking work. Ordinary typed next_effect inspection remains useful
when resource selection is unnecessary. Completing a read with a write result
is a compile error, not a result-enum case handled by application logic.

Journal now requires `(File<ReadAppend>, Effect<ReadSystemTime>)` and has no
filename configuration. The host selects its path at the single FileDriver
binding. Journal's parsing, failure mapping, append-outcome discard, and request
correlation remain application policy. Existing path-based filesystem Effects
remain available for the broader directory-resource use case.

## Alternatives

| Shape | Assessment |
| --- | --- |
| Independent append/read capabilities with a shared filename | Does not establish a shared Driver target. |
| One File operation enum and one result enum | Allows operation/result mismatches in controlled completion and weakens the typed outcome boundary. |
| One permission-typed resource issuing typed operations | Chosen: one binding identity with operation-specific results. |
| A distinct Rust marker type for each file | Makes callers invent identities and obstructs repeated reusable Assemblies. |

## Invariants and failure modes

- Model ownership, pure transitions, typed finite work, runtime-owned tasks,
  causal delivery, cancellation, and controlled determinism are unchanged.
- Permissions constrain operation construction, not OS access-control policy.
  Filesystem errors remain typed effect failures; binding does not touch disk.
- Missing files fail reads; append creates its target and parent directories.
  Sharing a file does not serialize effects or promise read-after-write order.
  Applications await outcomes when ordering matters.
- Resource identity remains stable through permission narrowing and cloning.
  Narrowing cannot add operations absent from the Program declaration.
- Write includes append, but narrowing to `AppendOnly` or `ReadAppend` removes
  replacement authority. Replacements share the same path binding and exact
  dispatch rules as reads and appends; failure and cancellation use the normal
  typed effect outcome and introduce no automatic retry.
- Live paths belong to the host binding. Controlled tests assert resource
  selection, operation data, and outcomes without needing a real path.
- No persistence, replay, persistent open handles, or generic resource-family
  extension point is introduced by this trial.

## Evidence and recovery

`tests/file_capabilities.rs` covers typed command emission, permission narrowing,
same-type resource isolation, foreign registration/issuance/selectors, controlled
completion and cancellation, and live same-file append/read causality. Rustdoc
compile-fail cases reject unavailable operations, permission widening, and
incorrect completion types. `src/file/tests.rs` rejects duplicate, foreign,
wrong-operation, missing, and type-wide fallback bindings. Repeated controlled
inputs produce identical traces. Write coverage checks replacement payloads,
permission narrowing, exact selection, success/error/cancellation, and live
replacement of existing contents through a single binding. Journal's existing domain tests are migrated
and its live tests use one FileDriver for each instance.

Rollback removes File<P> and exact-effect binding, restores Journal's separate
path descriptors and Driver tuple, and reverts the matching contracts/tests.
No persisted state or wire format changes. Existing journal contents remain
compatible; host paths must be preserved during migration or rollback.
