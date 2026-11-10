# Usage Journal

This document contains snippets of an abstract Samara implementation that may
or may not exist. The goal is to author the API we _wish_ to have, irrespective
of the one we do have.

## What Should a Component Own?

```rust
use std::path::Path;

use samara::{Component, Command};

/// The component marker type, used to spell `::<Settings>`
/// in various contexts like `Program` and `Runtime`.
pub struct Settings<T>;

/// The configuration, immutably passed to the `Component`
pub struct Config {
    write: EffectCapability<Write>,
}

/// The model, mutably passed to the `Component`
enum Model {
    Saving,
    Saved,
}

/// The message, how the `Component` reacts to the world.
enum Message<T> {
    Update(Box<dyn FnOnce(T)>)
    Updated,
}

impl<T> Component for Settings<T> {
    type Config = Config;
    type Model = Model;
    type Message = Message<T>;

    fn update(
        config: &Self::Config,
        model: &mut Self::Model,
        message: Self::Message
    ) -> Command<Self::Message> {
       Write::with_payload(&[0xbe, 0xef])
           .into_command_with(config.write, |_| Self::Message::Updated)

        todo!()
    }
}
```
