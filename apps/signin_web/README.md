# sign in, in the dom, under wasm-bindgen

A sign-in form lowered to elements — and mounted by a loader that is not
ours. Every other page here instantiates its own wasm with a classic
script (`glue.js`, `glue_dom.js`). This one is what an app looks like
when it is built by [trunk](https://trunkrs.dev) and loaded by
wasm-bindgen, the toolchain it picks when it also wants `web-sys`,
`fetch` and the crates written against them.

The border is the module glue, `crates/bunny_ui_web/glue/esm/bunny.js`.
wasm-bindgen resolves the wasm's `./bunny.js` and `./bunny_gpu.js`
imports beside its own loader, and the page hands the instance over
once:

```js
import { attachDom } from "./bunny.js";
addEventListener("TrunkApplicationStarted", async ({ detail }) => {
  await attachDom(detail.wasm.memory, detail.wasm, (width, height, scale) =>
    window.wasmBindings.boot(width, height, scale),
  );
});
```

`attachDom` imports `glue_dom.js` and borrows the element shell whole —
the patch applier, the element registry, the delegated clicks, the
inputs' editing — so the mode has one implementation whichever loader
made the instance. The mount is the element with id `app`, as on every
element page. Three files travel together beside the page: `bunny.js`,
`bunny_gpu.js` and `glue_dom.js` (`web/index.html` copies them out of
the crate).

The form says what a browser needs to fill it. Each box names what it
holds and what a reader calls it, and the password is a secret:

```rust
text_field("", email)
    .autocomplete(Autocomplete::Username)
    .accessibility_label("Email");
text_field("", password)
    .secret(true)
    .autocomplete(Autocomplete::CurrentPassword)
    .accessibility_label("Password");
```

which reaches the page as `<input type="text" autocomplete="username"
aria-label="Email">` and `<input type="password"
autocomplete="current-password" aria-label="Password">` — the pair a
password manager recognizes. "Show password" flips the secret off and
the input is retyped in place, value and caret kept.

The page opens with the keyboard in the email box (`.auto_focus()`), Enter
in either box signs in (`.on_submit`), and Tab reaches the checkbox and the
button — a `role="checkbox"` and a `role="button"` with a tab stop — which
Space and Enter activate.

Build and run (trunk fetches the wasm-bindgen CLI `web/Trunk.toml`
pins, which must match the crate in `Cargo.lock`):

```
cd apps/signin_web/web
trunk serve
```

Then open http://127.0.0.1:8874.
