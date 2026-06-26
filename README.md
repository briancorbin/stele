# Stele

**JSON-first, type-safe i18n codegen with pluggable per-language emitters.**

One JSON catalog in. Idiomatic, fully-typed accessor code out — for *every* language your product ships in. Think "protobuf for your copy."

> A *stele* is an inscribed stone slab. The Rosetta Stone is a stele: one decree, carved in three scripts. Stele does the same thing for your app's strings — one source of truth, rendered into many languages of code.

---

## The problem

You want translation files as plain JSON, because that's what's easy to hand to translators. But you don't want to write `t("home.greeting")` all over your code — it's a magic string the compiler can't check, and it's ugly.

Existing tools each solve half of this:

- **typesafe-i18n** gives you nested typed accessors, but ships a runtime and is web-shaped.
- **Paraglide** compiles to typed functions, but they're flat and web-shaped.
- **SwiftGen / R.swift** are typed, but Swift-only.
- **TMS platforms** (Tolgee, Crowdin, …) manage translations, then hand you *stringly-typed* native resources.

Nobody takes **one** JSON catalog and emits **type-safe, idiomatic, zero-runtime** accessors across **many** languages. That's Stele.

## What you get

Author your copy as plain, translator-friendly JSON:

```json
{
  "home": {
    "title": "Sidewalk",
    "greeting": "Hey {name}, there are dogs nearby",
    "nearby": {
      "$plural": {
        "one": "{count} dog within {radius} of you",
        "other": "{count} dogs within {radius} of you"
      }
    }
  }
}
```

Run `stele generate`, and call into it with full autocomplete and compile-time safety — in whichever language you're writing:

**TypeScript**
```ts
const copy = createCopy("en");
copy.home.title;                              // "Sidewalk"
copy.home.greeting({ name: "Brian" });        // typed; { nmae } is a compile error
copy.home.nearby({ count: 3, radius: "200m" });
```

**Swift**
```swift
let copy = Copy(.en)
copy.home.title                               // "Sidewalk"
copy.home.greeting(name: "Brian")             // idiomatic labeled args
copy.home.nearby(count: 3, radius: "200m")
```

Same catalog. Same structure. Each target gets the shape a native of that language expects — and in both, a typo'd parameter, a missing argument, a nonexistent key, or a wrong type is a **compile error**, not a runtime surprise.

## Design principles

- **JSON-first.** The JSON is the source of truth. Generated code is build output you can commit and review in PRs.
- **Zero-runtime output.** Stele emits plain code — no proxy, no resolver, no runtime dependency. Tree-shakeable, bundler-friendly.
- **Hermes-safe by construction.** Plural rules are resolved at *generate* time and baked into a static table. The emitted code never calls `Intl.PluralRules` — which matters because React Native's Hermes engine doesn't have it.
- **Pluggable emitters.** A language-neutral, serializable IR is the contract. Adding a language is one emitter against that IR — the core never changes.

## How it works

```
locales/*.json  ──▶  parse  ──▶  IR (serializable)  ──▶  emitter  ──▶  generated code
                                     │
                                     ├─▶ TypeScript
                                     ├─▶ Swift
                                     └─▶ … (your language here)
```

The IR is the seam. `stele ir` will print it as JSON, which means emitters can even be written in their own target language (the protoc-plugin model).

## Install

The `stele` binary ships as a native package — no Rust toolchain needed. Add it
and run it as a build step:

```bash
pnpm add -D @stelegen/cli        # or: npm i -D @stelegen/cli
```

The right prebuilt binary for your platform is pulled in automatically (macOS
arm64/x64, Linux x64/arm64, Windows x64). Other ways in:

```bash
cargo install stelegen           # Rust users — installs the `stele` binary
cargo build --release            # from a clone — ./target/release/stele
```

## Quickstart

Point `stele` at a config and generate, then commit the output and import it.

```bash
stele generate                   # reads ./stele.toml
stele ir --locales locales --canonical en   # inspect the IR
```

`stele.toml`:

```toml
canonical = "en"
locales   = "locales"

[[target]]
lang = "typescript"
out  = "src/copy.gen.ts"
# callable = true               # emit no-arg leaves as () => "..." thunks

[[target]]
lang = "swift"
out  = "Sources/Copy.swift"
```

Wire it into your build so it can't drift:

```jsonc
// package.json
"scripts": { "copy:gen": "stele generate" }
```

A CI check keeps generated output honest: `stele generate && git diff --exit-code`.

A worked example — input, config, and generated output for both languages — lives in [`examples/`](examples/).

## Status

Early, but real. Both emitters are verified end-to-end: the generated code compiles, runs, and rejects bad calls at compile time.

- [x] JSON → serializable IR
- [x] TypeScript emitter (nested typed accessors, zero-runtime)
- [x] Swift emitter (idiomatic labeled args, nested structs)
- [x] **ICU4X**-backed plurals — authoritative CLDR rules baked into per-locale
      tables at generate time, validated against the oracle, emitted as pure
      lookups (correct `one`/`few`/`many` for Polish, Arabic, Russian, …; no
      runtime `Intl.PluralRules`, so it's Hermes-safe)
- [x] Distribution — native binary via npm (`@stelegen/cli`) and crates.io (`stelegen`),
      cross-compiled for macOS/Linux/Windows by CI on each tag
- [ ] More emitters (Kotlin, Go, Rust, Java)
- [ ] `$select` (gender / arbitrary branching)
- [ ] Validate `$plural` coverage against each locale's CLDR category set

## License

MIT © 2026 Brian Corbin
