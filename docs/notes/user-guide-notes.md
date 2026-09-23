# User Guide Notes

Loose material for the future Samara user guide.

- **Choose Effect boundaries by control-flow significance.** Shane's guiding
  principle, September 22, 2026:

  > User-defined effects should sit at the lowest possible boundary, but not be
  > single IO jobs. That is, low enough that if it errors, the error is as
  > specific as would ever influence control flow.

  One Effect can describe a cohesive, finite interaction spanning several I/O
  operations. Its typed errors should preserve the distinctions that could
  change the caller's next action. Internal steps need not become separate
  Commands and Messages when the caller has no decisions to make between them.

  Tight GPIO sequencing is a motivating example: declare one typed Effect and
  implement its live Driver using straight-line async/await for pin operations
  and required delays. The sequence remains runtime-owned work; its terminal
  outcome returns through a Message, without async code mutating the Model.

  Develop the distinction between implementing a device operation's mechanism
  and hiding application retry or workflow policy inside a Driver. Also explain
  the testing boundary: controlled execution supplies the whole Effect's
  outcome; its internal steps need their own Driver tests, or a finer boundary
  if application logic must observe or coordinate them. See the existing
  [Layer/Driver distinction](../architecture/effects-layering.md).

- **Hidden application-state management suggests the Effect's aperture is
  wrong.** Workshop wording, not yet a finished rule:

  > If implementing an Effect requires smuggling application-state management
  > or mutation into its Driver, reconsider the Effect's boundary.

  Clarify what we mean by "pure": the descriptor is inert data and its outcome
  mapper is pure, while the live Driver is explicitly side-effecting. Changing
  GPIO pins or writing a requested configuration file is the intended world
  interaction. Owning application workflow state or bypassing Model/Message
  transitions is a different responsibility. Application coordination belongs
  in Components; reusable compositional policy belongs in Layers.

  This is not a ban on local variables or persistent operational state. A
  Driver may need buffers, a pooled client, or a device handle to perform its
  declared interaction. Workshop examples that distinguish that mechanism from
  a hidden application state machine. See
  [persistent Driver state](first-real-application.md#persistent-state-in-manual-effect-drivers).

  The API separates declaration from execution and gives the Driver a
  descriptor rather than a Component Model. It does not prove purity or prevent
  arbitrary Rust code from hiding shared application state behind a handle.
  This remains an architecture and review principle worth teaching explicitly.

- **Bundled Effects are reference designs for UDEs.** They should meet the same
  boundary, typed-outcome, and controlled-execution standards. Their granularity
  teaches users what to put in a Driver and what to keep in application logic;
  review the first-party catalog alongside custom examples when workshopping
  these principles.
