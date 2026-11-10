---
theme: default
title: The Samara Framework
info: Side-effect free programming.
class: text-center
drawings:
  persist: false
transition: slide-left
comark: true
---

# The Samara Framework

Side-effect free programming.

---

# Components

````md magic-move {lines: true}
```rs {*|2|3|5|*}
trait Component {
    type Message;
    type Model;

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message>;
}
```

```rs
impl Component for MyComponent {
    type Message = MyMessage;
    type Model = MyModel;

    fn update(&self, model: &mut MyModel, message: MyMessage) -> Command<MyMessage> {
        Command::none()
    }
}
```
````

---

# Messages and Models

````md magic-move {lines: true}
```rs
struct MyModel;
```

```rs
struct MyModel {
    waiting: bool
}
```

```rs
struct MyModel {
    waiting: bool
}

enum MyMessage {}
```

```rs
struct MyModel {
    waiting: bool
}

enum MyMessage {
    Ping,
}
```

```rs
struct MyModel {
    waiting: bool
}

enum MyMessage {
    Ping,
    Pong,
}
```
````

---

# Updating

````md magic-move {lines: true}
```rs
impl Component for MyComponent {
    type Message = MyMessage;
    type Model = MyModel;

    fn update(&self, model: &mut MyModel, message: MyMessage) -> Command<MyMessage> {
        Command::none()
    }
}
```

```rs
impl Component for MyComponent {
    type Message = MyMessage;
    type Model = MyModel;

    fn update(&self, model: &mut MyModel, message: MyMessage) -> Command<MyMessage> {
        match (model, message) {
            (MyModel { waiting }, MyMessage::Ping) => todo!(),
            (Mymodel { waiting }, MyMessage::Pong) => todo!(),
        }
    }
}
```

```rs {*|8|9|11-12|*}
impl Component for MyComponent {
    type Message = MyMessage;
    type Model = MyModel;

    fn update(&self, model: &mut MyModel, message: MyMessage) -> Command<MyMessage> {
        match (model, message) {
            (MyModel { waiting }, MyMessage::Ping) => {
                if waiting {
                    Command::none()
                } else {
                    waiting = true;
                    Command::after(Duration::from_secs(1), MyMessage::Pong)
                }
            },
            (Mymodel { waiting }, MyMessage::Pong) => todo!(),
        }
    }
}
```

```rs {*|16|17|19-20|*}
impl Component for MyComponent {
    type Message = MyMessage;
    type Model = MyModel;

    fn update(&self, model: &mut MyModel, message: MyMessage) -> Command<MyMessage> {
        match (model, message) {
            (MyModel { waiting }, MyMessage::Ping) => {
                if waiting {
                    Command::none()
                } else {
                    waiting = true;
                    Command::after(Duration::from_secs(1), MyMessage::Pong)
                }
            },
            (Mymodel { waiting }, MyMessage::Pong) => {
                if !waiting {
                    Command::none()
                } else {
                    waiting = false;
                    println!("Done waiting!");
                }
            }
        }
    }
}
```
````
