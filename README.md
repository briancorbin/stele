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
    "greeting": "Hey {{name}}, there are dogs nearby",
    "nearby": {
      "$plural": {
        "one": "{{count}} dog within {{radius}} of you",
        "other": "{{count}} dogs within {{radius}} of you"
      }
    }
  }
}
```

Placeholders use the `{{name}}` double-brace convention (whitespace tolerant — `{{ name }}` works too). Literal single braces in copy are left untouched, so `"set it to {0}"` is safe.

Run `stele generate`, and call into it with full autocomplete and compile-time safety — in whichever language you're writing:

**TypeScript**
```ts
const stele = createStele("en");
stele.home.title;                              // "Sidewalk"
stele.home.greeting({ name: "Brian" });        // typed; { nmae } is a compile error
stele.home.nearby({ count: 3, radius: "200m" });
```

**Swift**
```swift
let stele = Stele(.en)
stele.home.title                               // "Sidewalk"
stele.home.greeting(name: "Brian")             // idiomatic labeled args
stele.home.nearby(count: 3, radius: "200m")
```

The accessor is named `stele` by default — `import { stele }` and call `stele.home.greeting(...)`, the way you `import { z } from "zod"` and write `z.string()`. One config line (`binding`) renames it to whatever you like (`copy`, `t`, …), uniformly across every target.

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
stele check                      # validate the catalog across locales (CI gate)
stele ir --locales locales --canonical en   # inspect the IR
```

`stele.toml`:

```toml
canonical = "en"
locales   = "locales"

[[target]]
lang = "typescript"
out  = "src/stele.gen.ts"
# callable = true               # emit no-arg leaves as () => "..." thunks
# case = "camel"                # output identifier case (see below)
# binding = "stele"             # brand name for the API (see below)

[[target]]
lang = "swift"
out  = "Sources/Stele.swift"
```

### Locale layout

A locale can be one flat file, or split across files and folders — the path
becomes the namespace, so you organize copy however your app is organized:

```
locales/
  en/
    nav.json            →  stele.nav.*
    walker/
      today.json        →  stele.walker.today.*
      schedule.json     →  stele.walker.schedule.*
  es/
    nav.json
    ...
```

`locales/en.json` (one file) and `locales/en/` (a folder tree) both work, and may
even be mixed; everything for a locale is deep-merged. A key defined twice across
files is an error, never a silent clobber.

### Key casing

Author keys in **any** case — `walker_today`, `walkerToday`, `walker-today` all
parse the same. The `case` option on a target picks the *output* identifier case,
applied uniformly to namespaces, leaves, and params:

| `case` | `walker_today` → | param `first_name` → |
|---|---|---|
| `camel` (default) | `stele.walkerToday` | `{ firstName }` |
| `snake` | `stele.walker_today` | `{ first_name }` |
| `pascal` | `stele.WalkerToday` | `{ FirstName }` |
| `preserve` | verbatim | verbatim |

If two keys collapse to the same name under the chosen case (`dog_count` and
`dogCount`), or a key isn't a valid identifier (`2fa`), generation fails loudly
rather than emitting broken or silently-clobbered code.

### API binding name

The generated API is branded `stele` by default — the accessor type is `Stele`,
the factory is `createStele`, and the `react` target exports `useStele` /
`SteleProvider`. The `binding` option on a target renames that whole surface with
one word, applied uniformly across every language:

| `binding` | TypeScript / Swift | React |
|---|---|---|
| `stele` (default) | `createStele` / `Stele` | `useStele`, `SteleProvider` |
| `copy` | `createCopy` / `Copy` | `useCopy`, `CopyProvider` |
| `t` | `createT` / `T` | `useT`, `TProvider` |

It's purely cosmetic — the emitted code is identical apart from those names — so a
team can pick the word that reads best at their call sites without any lock-in.

Wire it into your build so it can't drift:

```jsonc
// package.json
"scripts": { "copy:gen": "stele generate" }
```

A CI check keeps generated output honest: `stele generate && git diff --exit-code`.

A worked example — input, config, and generated output for both languages — lives in [`examples/`](examples/).

### The locale store

The core factory (`createStele(locale)`) is pure — you pass the locale in. For an
app you usually want *one* active locale that any code can read or change, that
persists across launches, and that re-renders the UI when it flips. That's the
`store` target: a tiny framework-agnostic module that owns the active locale and
is the single source of truth.

```toml
[[target]]
lang = "typescript"
out  = "src/stele.gen.ts"

[[target]]
lang = "store"
out  = "src/stele.store.ts"
core = "./stele.gen"          # import path to the typescript target's output
```

```ts
import {
  getStele, getLocale, setLocale, subscribeLocale,  // read / write / observe
  followDevice, isFollowingDevice, syncDevice,        // device / "system" mode
  resolveLocale, initLocale,                          // startup helpers
} from "./stele.store";

getStele().home.greeting({ name });   // accessor bound to the active locale
getLocale();                          // "en"
setLocale("es");                      // pin Spanish — stops following the device, persists
followDevice();                       // back to following the device locale, persists "system"
```

The store tracks a **preference**, not just a locale: either `"system"` (follow
the device) or a pinned `Locale`. That's the difference between "remember my
choice" and "follow my phone" — keeping them distinct is what avoids the classic
bug where a once-saved locale shadows the device forever.

- **`resolveLocale(tags)`** maps arbitrary BCP-47 device tags to a supported
  `Locale` (`["es-MX","en-US"] → "es"`), falling back to the canonical locale.
- **`initLocale({ storage, deviceLocales })`** is the one-call startup: restore the
  saved preference (or default to `"system"`), wire the device-locale source, and
  apply it. Persistence is a pluggable adapter (`LocaleStorage`) storing
  `"system" | Locale` — *you* hand it AsyncStorage / localStorage, so the store
  pulls in no platform dependency.
- **`syncDevice()`** re-reads the device locale *only while in system mode* — wire
  it to an `AppState` "active" listener to track the OS language changing live.

```ts
// once, before your first render (e.g. behind a splash screen):
await initLocale({
  storage: {                                          // ~3 lines you provide
    load: () => AsyncStorage.getItem("locale") as Promise<LocalePref | null>,
    save: (p) => AsyncStorage.setItem("locale", p),
  },
  deviceLocales: () => Localization.locales,          // expo-localization, navigator.languages, …
});

// follow the device with no persistence at all? just:
await initLocale({ deviceLocales: () => Localization.locales });
```

### React / React Native

Add a `react` target to bind the store to React via `useSyncExternalStore` —
hooks only, **no Provider to mount**. `setLocale` from anywhere (a component or
not) re-renders every `useStele()` consumer. Works the same on web React and
React Native (no JSX build step).

```toml
[[target]]
lang = "react"
out  = "src/stele.react.ts"
store = "./stele.store"       # import path to the store target's output
```

```tsx
import { useStele, useLocale, useFollowingDevice } from "./stele.react";

// in a component — re-renders when the locale changes:
const stele = useStele();
const [locale, setLocale] = useLocale();
const onSystem = useFollowingDevice();   // for a "System" radio in settings
return <Text onPress={() => setLocale("es")}>{stele.home.greeting({ name })}</Text>;
```

### As a node package (no files in your repo)

Instead of loose `.ts` files, Stele can emit a **compiled package** — `.js` +
`.d.ts` + `package.json` — via a `[package]` block. Point `out` straight at
`node_modules/<name>/` and Stele writes a real, import-by-name package there;
nothing lands in your source tree. It's the "codegen as a dependency" model, à la
Prisma Client.

```toml
[package]
name  = "@myapp/copy"
out   = "node_modules/@myapp/copy"
store = true
react = true
```

```jsonc
// package.json — regenerate on install (see the caveat below)
"scripts": { "postinstall": "stele generate", "copy:gen": "stele generate" }
```

```ts
import { createStele }          from "@myapp/copy";
import { useStele, useLocale }  from "@myapp/copy/react";
import { getStele, initLocale } from "@myapp/copy/store";
```

Stele emits the `.js` and `.d.ts` **directly** — no `tsc` in the loop, it stays
one binary. `tsc` trusts the `.d.ts`, and the package resolves by name through its
`exports` map (`.`, `./store`, `./react`). A generated example lives in
[`examples/out/pkg/`](examples/out/pkg/).

> **`node_modules` is the package manager's turf.** `npm` / `pnpm` / `yarn install`
> prune directories they didn't create, so you must regenerate on `postinstall`.
> Under pnpm's strict store this is the same pattern — and the same caveats — as
> Prisma Client; Metro's resolver tends to tolerate it well for React Native. If
> your setup fights it, point `out` at a gitignored folder and add one resolver
> alias instead — same artifact, different home.

### Validate the catalog — `stele check`

Generation makes the *current* locale type-safe; `stele check` keeps the whole
catalog honest as you add locales. It compares every locale against the canonical
one and exits non-zero on problems — wire it into CI:

```bash
stele check            # errors fail; warnings don't
stele check --strict   # warnings fail too
```

It catches:

- **missing translations** — a canonical key absent in another locale *(error)*
- **placeholder drift** — `{{nombre}}` where the canonical says `{{name}}` would
  be `undefined` at runtime *(error)*; a canonical placeholder a translation drops
  *(warning)*
- **plural-coverage gaps** — a locale missing a category its CLDR rules require
  (`few`/`many` for Polish, …), which would render a blank string *(error)*
- **kind mismatches** — a key that's a plain string in one locale and a `$plural`
  in another *(error)*
- **malformed `$plural`** and **stale keys** not in the canonical locale

```
stele check — 3 locales, 657 keys (canonical: en)

  ✓ en
  ✗ es — 1 error(s), 1 warning(s)
      error    home.greeting  —  placeholder {{nombre}} is not in the canonical string — it will be undefined at runtime
      warning  home.tagline   —  key is not in the canonical locale — it will be ignored

✗ check failed — 1 error(s), 1 warning(s)
```

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
- [x] Multi-file / folder locales (path-as-namespace, deep-merged)
- [x] Locale store (`store` target) — framework-agnostic single source of truth:
      `getLocale` / `setLocale` / `subscribeLocale` / `resolveLocale` /
      `initLocale`, observable + pluggable persistence, zero platform deps
- [x] Device / "system" mode — store tracks a `"system" | Locale` preference
      (`followDevice` / `isFollowingDevice` / `syncDevice`), so "follow the phone"
      and "remember my choice" stay distinct
- [x] React / React Native bindings (`react` target) — `useStele` / `useLocale` /
      `useFollowingDevice` bound to the store via `useSyncExternalStore`, no
      Provider, `setLocale` from anywhere re-renders consumers
- [x] `{{name}}` placeholders (double-brace, whitespace tolerant, literal-`{}`-safe)
- [x] Any-input / chosen-output key casing (`case` option) with collision + invalid-id checks
- [x] Configurable API binding name (`binding` option) — `stele.*` by default, one word renames the whole surface
- [x] Node package output (`[package]`) — compiled `.js` + `.d.ts` + `package.json`
      (import-by-name, `exports` map) instead of loose `.ts`; emit into
      `node_modules/<name>` for a zero-repo-footprint "codegen as a dependency" setup
- [x] `stele check` — cross-locale validation (missing/stale keys, placeholder
      drift, plural-category coverage, kind mismatches); non-zero exit for CI
- [ ] `$select` (gender / arbitrary branching)
- [ ] More emitters (Kotlin, Go, Rust, Java)

## License

MIT © 2026 Brian Corbin
