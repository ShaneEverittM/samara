# Samara presentation draft

The active deck is [slides.md](slides.md). It introduces Components, Messages,
Models, and updates using animated Rust examples.

This is a work in progress: the examples use the earlier `Component` API and
contain unfinished code. They are presentation snippets, not compiled examples;
see [the API guidance](../../api-guidance.md) for the current API.

From this directory:

```sh
bun install --frozen-lockfile
bun run dev
```

To build the active deck into `dist/`:

```sh
bun run build
```

`slides.old.md` preserves the original Slidev starter template as an authoring
reference. It is not the active deck and refers to starter assets that are not
included here.
