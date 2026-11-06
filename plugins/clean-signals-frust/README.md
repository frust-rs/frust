# clean-signals-frust

Frust integration for [clean-signals](https://crates.io/crates/clean-signals):
component-scoped controllers and failure listeners, so a Frust view can drive
clean-signals use cases and react to their failures without hand-written glue.

## Dependencies

```toml
[dependencies]
clean-signals = "0.1"
clean-signals-frust = "0.6"
frust = { package = "frust-ui", version = "0.6" }
```

The Frust facade crate is published on crates.io as `frust-ui` because the name
`frust` is already taken on the registry
(<https://github.com/lloydmeta/frunk/issues/258>). Its library name is still
`frust`, so code keeps writing `use frust::...`; the `package = "frust-ui"` key
is what maps the dependency to the published crate.

Never enable the `effects` feature of `clean-signals`; the version requirement
must stay identical across every crate in your graph so they share one crate
identity.

## Localized failure text

`async_view` and `use_failure_listener` never render failure text themselves;
the closure you pass decides. `Failure::user_message()` is the non-localized
default (the failure's `Display`). To localize, read your i18n handle in the
component's `build`, move it into the closure, and render through an
app-defined method, for example
`move |f: AppFailure| toasts.error(f.localized(&i18n))`, where `localized` maps
each variant to a message key and resolves it with `i18n.t(..)`. This crate
takes no i18n dependency.

## Scaffolding

`frust create --arch clean-signals` generates a project with these dependency
lines already in place.

## Licence

MIT.
