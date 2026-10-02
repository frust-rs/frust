# frust-cupertino

The Cupertino (iOS-styled) design-system catalog for [Frust](https://frust.dev):
`CupertinoNavBar`, `CupertinoTabBar`, `CupertinoSwitch`, `CupertinoAlertDialog`,
`CupertinoActionSheet`, `CupertinoActivityIndicator` and `CupertinoButton`, plus
the Cupertino theme and its "Liquid Glass" recipe. Every widget is re-exported
flat at the crate root.

It is built on the `frust` facade's public `authoring` surface only. Applications
depend on `frust-cupertino` directly, next to `frust`; the facade does not
re-export it. The crate bundles no fonts and no third-party code.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

The crate's SPDX expression is `MIT OR Apache-2.0`: choose either
[LICENSE-MIT](LICENSE-MIT) or [LICENSE-APACHE](LICENSE-APACHE).
