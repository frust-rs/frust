//! The identity gate (hazard row D): a patch that changes a component's
//! `State` type cannot take effect, because the jump table keys the seam on
//! its `call_it` monomorphisation and a new `State` is a new symbol the old
//! callers never reach.
//!
//! The seam is the private hot function `frust_core::hotpatch::checked_build::<C>`
//! behind `build_erased::<C>`, called through `HotFunction::call_it`. Its v0
//! symbol demangles to
//! `<frust_core::hotpatch::checked_build<C> as frust_hotpatch::hot_fn::HotFunction<(&C, &mut C::State, frust_core::hotpatch::SeamWitness), _>>::call_it`,
//! so the argument tuple names `C::State` concretely. [`SeamSet::from_inputs`]
//! collects one [`SeamInstance`] per component defined in a build's objects.
//! [`check`] compares a candidate with the accepted seam set (the base's
//! instances plus every accepted patch's, grown by [`merge`]): a component
//! whose argument tuple differs is a [`StateTypeChanged`]. A component the set
//! has not seen is new and passes; its first instance is what later patches
//! are compared with. The [`SeamReport`] also carries the candidate's whole
//! set, so the session can tell whether a missed jump-table key names a seam
//! instance the patch contains. Legacy-mangled seam symbols fail closed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use object::{Object as _, ObjectSymbol as _};
use serde::{Deserialize, Serialize};

use super::HotpatchError;
use super::layout::for_each_object;

/// The seam's hot function, as its demangled path starts.
pub const SEAM_FN: &str = "frust_core::hotpatch::checked_build";

/// The trait method whose monomorphisation is the jump-table key.
const CALL_IT: &str = "call_it";

/// One component's seam instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeamInstance {
    /// `C`, e.g. `my_app::HomePage`.
    pub component: String,
    /// The `HotFunction` argument tuple, e.g.
    /// `(&my_app::HomePage, &mut my_app::HomeState, frust_core::hotpatch::SeamWitness)`.
    pub args: String,
    /// The raw (mangled) symbols defining this instance.
    pub symbols: BTreeSet<String>,
}

/// Seam instances keyed by component.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeamSet {
    pub instances: BTreeMap<String, SeamInstance>,
}

impl SeamSet {
    /// The seam instances defined in `inputs` (rlibs or objects).
    pub fn from_inputs(inputs: &[PathBuf]) -> Result<Self, HotpatchError> {
        let mut names = Vec::new();
        for input in inputs {
            for_each_object(input, |_, file| {
                names.extend(
                    file.symbols()
                        .filter(|symbol| symbol.is_definition())
                        .filter_map(|symbol| symbol.name().ok().map(str::to_string)),
                );
                Ok(())
            })?;
        }
        Self::from_symbols(names.iter().map(String::as_str))
    }

    /// The seam instances among `symbols` (mangled names; others ignored).
    pub fn from_symbols<'a>(
        symbols: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, HotpatchError> {
        let mut set = Self::default();
        for symbol in symbols {
            let Ok(demangled) = rustc_demangle::try_demangle(symbol) else {
                continue;
            };
            let text = format!("{demangled:#}");
            let Some((component, args)) = parse_seam(&text)? else {
                continue;
            };
            let bare = symbol.trim_start_matches('_');
            if !bare.starts_with('R') {
                return Err(HotpatchError::unsupported(format!(
                    "seam symbol `{symbol}` is not v0-mangled; the identity gate needs \
                     `-C symbol-mangling-version=v0`"
                )));
            }
            set.insert(component, args, symbol)?;
        }
        Ok(set)
    }

    fn insert(
        &mut self,
        component: String,
        args: String,
        symbol: &str,
    ) -> Result<(), HotpatchError> {
        let instance = self
            .instances
            .entry(component.clone())
            .or_insert_with(|| SeamInstance {
                component: component.clone(),
                args: args.clone(),
                symbols: BTreeSet::new(),
            });
        if instance.args != args {
            return Err(HotpatchError::unsupported(format!(
                "two seam instances for `{component}` in one build: `{}` and `{args}`",
                instance.args
            )));
        }
        instance.symbols.insert(symbol.to_string());
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn get(&self, component: &str) -> Option<&SeamInstance> {
        self.instances.get(component)
    }

    /// True when `symbol` (mangled) is one of this set's seam instances.
    pub fn contains_symbol(&self, symbol: &str) -> bool {
        self.instances
            .values()
            .any(|instance| instance.symbols.contains(symbol))
    }
}

/// A component whose seam argument tuple changed: its `State` type is not
/// the accepted one. The session's `RestartRequired { StateTypeChanged { .. } }`
/// reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTypeChanged {
    pub component: String,
    pub accepted_args: String,
    pub candidate_args: String,
}

impl fmt::Display for StateTypeChanged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}'s State changed type: {} → {}",
            self.component, self.accepted_args, self.candidate_args
        )
    }
}

/// [`check`]'s verdict plus the candidate's seam instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamReport {
    /// Empty when the candidate passes.
    pub changed: Vec<StateTypeChanged>,
    /// Every seam instance present in the candidate.
    pub present: SeamSet,
}

/// Compares `candidate` with the accepted seam set.
pub fn check(candidate: SeamSet, accepted: &SeamSet) -> SeamReport {
    let changed = candidate
        .instances
        .values()
        .filter_map(|instance| {
            let known = accepted.instances.get(&instance.component)?;
            (known.args != instance.args).then(|| StateTypeChanged {
                component: instance.component.clone(),
                accepted_args: known.args.clone(),
                candidate_args: instance.args.clone(),
            })
        })
        .collect();
    SeamReport {
        changed,
        present: candidate,
    }
}

/// Adds the candidate's new components to the accepted seam set. An existing
/// component keeps its accepted argument tuple; its symbols are unioned.
pub fn merge(accepted: &mut SeamSet, candidate: &SeamSet) {
    for (component, instance) in &candidate.instances {
        match accepted.instances.get_mut(component) {
            None => {
                accepted
                    .instances
                    .insert(component.clone(), instance.clone());
            }
            Some(known) if known.args == instance.args => {
                known.symbols.extend(instance.symbols.iter().cloned());
            }
            Some(_) => {}
        }
    }
}

/// Parses a demangled (`{:#}`) symbol: `Some((component, args))` for a seam
/// `call_it` instance, `None` for any other symbol. A symbol that names the
/// seam's `HotFunction::call_it` in a shape not understood is
/// [`HotpatchError::BuilderUnsupported`].
pub fn parse_seam(demangled: &str) -> Result<Option<(String, String)>, HotpatchError> {
    let Some(rest) = demangled
        .strip_prefix('<')
        .and_then(|r| r.strip_prefix(SEAM_FN))
    else {
        return Ok(None);
    };
    let rest = rest.strip_prefix("::").unwrap_or(rest);
    let Some(rest) = rest.strip_prefix('<') else {
        return Ok(None);
    };
    let surprise = || {
        HotpatchError::unsupported(format!(
            "seam symbol `{demangled}` has a shape the identity gate does not understand"
        ))
    };
    let (component, rest) = split_balanced(rest).ok_or_else(surprise)?;
    let Some(rest) = rest.strip_prefix(" as ") else {
        return Ok(None);
    };
    let open = rest.find('<').ok_or_else(surprise)?;
    if !rest[..open].ends_with("::HotFunction") {
        return Ok(None);
    }
    let (trait_args, rest) = split_balanced(&rest[open + 1..]).ok_or_else(surprise)?;
    let Some(method) = rest.strip_prefix(">::") else {
        return Err(surprise());
    };
    if method != CALL_IT && !method.starts_with(&format!("{CALL_IT}.")) {
        return Ok(None);
    }
    let args = first_argument(trait_args).ok_or_else(surprise)?;
    if component.is_empty() || !args.starts_with('(') {
        return Err(surprise());
    }
    Ok(Some((component.to_string(), args.to_string())))
}

/// Splits `text` after an opening `<` at its matching `>`: the enclosed
/// text and what follows the `>`. `->` in a fn type is not a bracket.
fn split_balanced(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let mut previous = '\0';
    for (at, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' if previous == '-' => {}
            '>' | ')' | ']' if depth == 0 => {
                return (c == '>').then(|| (&text[..at], &text[at + 1..]));
            }
            '>' | ')' | ']' => depth -= 1,
            _ => {}
        }
        previous = c;
    }
    None
}

/// The first top-level comma-separated argument of `args`.
fn first_argument(args: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut previous = '\0';
    for (at, c) in args.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' if previous == '-' => {}
            '>' | ')' | ']' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => return Some(args[..at].trim()),
            _ => {}
        }
        previous = c;
    }
    Some(args.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `call_it` seam symbol, from the fixture app built with the
    /// pinned toolchain (Mach-O adds the leading `_`).
    const MANGLED: &str = "_RNvXNtCsfxYKN7w6hnv_14frust_hotpatch6hot_fnINvNtCskhDZPiJ344Y_10frust_core8hotpatch13checked_buildNtCskJOlR6liO6y_20hotpatch_fixture_app8HomePageEINtB2_11HotFunctionTRB1y_QNtB1A_9HomeStateNtBI_11SeamWitnessENtB2_9Fn3MarkerE7call_itB1A_";

    const DEMANGLED: &str = "<frust_core::hotpatch::checked_build<my_app::HomePage> as frust_hotpatch::hot_fn::HotFunction<(&my_app::HomePage, &mut my_app::HomeState, frust_core::hotpatch::SeamWitness), frust_hotpatch::hot_fn::Fn3Marker>>::call_it";

    fn unsupported<T: fmt::Debug>(result: Result<T, HotpatchError>) -> String {
        match result {
            Err(HotpatchError::BuilderUnsupported { detail }) => detail,
            other => panic!("expected BuilderUnsupported, got {other:?}"),
        }
    }

    fn set(entries: &[(&str, &str)]) -> SeamSet {
        let mut set = SeamSet::default();
        for (component, args) in entries {
            set.insert(
                component.to_string(),
                args.to_string(),
                &format!("_R{component}"),
            )
            .unwrap();
        }
        set
    }

    /// A path in the fixture app crate, which [`MANGLED`] was taken from.
    fn app(path: &str) -> String {
        format!("hotpatch_fixture_app::{path}")
    }

    // Parsing.

    #[test]
    fn a_seam_call_it_parses_to_component_and_argument_tuple() {
        assert_eq!(
            parse_seam(DEMANGLED).unwrap(),
            Some((
                "my_app::HomePage".to_string(),
                "(&my_app::HomePage, &mut my_app::HomeState, frust_core::hotpatch::SeamWitness)"
                    .to_string()
            ))
        );
    }

    #[test]
    fn a_turbofish_and_a_fn_type_argument_parse() {
        let demangled = "<frust_core::hotpatch::checked_build::<a::List<fn(u8) -> u8>> as h::HotFunction<(&a::List<fn(u8) -> u8>, &mut (u32, [u8; 2]), w::SeamWitness), h::Fn3Marker>>::call_it";
        assert_eq!(
            parse_seam(demangled).unwrap(),
            Some((
                "a::List<fn(u8) -> u8>".to_string(),
                "(&a::List<fn(u8) -> u8>, &mut (u32, [u8; 2]), w::SeamWitness)".to_string()
            ))
        );
    }

    #[test]
    fn other_symbols_are_not_seams() {
        for other in [
            "frust_core::hotpatch::checked_build::<my_app::HomePage>",
            "frust_core::hotpatch::build_erased::<my_app::HomePage>",
            "<frust_core::hotpatch::checked_build_all<a::B> as h::HotFunction<(u8,), M>>::call_it",
            "<frust_core::hotpatch::checked_build<a::B> as core::ops::function::FnOnce<(&a::B,)>>::call_once",
            "<frust_core::hotpatch::checked_build<a::B> as h::HotFunction<(&a::B,), M>>::other",
            "<my_app::HomePage as frust_core::Component>::build",
        ] {
            assert_eq!(parse_seam(other).unwrap(), None, "{other}");
        }
    }

    #[test]
    fn a_seam_in_an_unexpected_shape_is_builder_unsupported() {
        for odd in [
            "<frust_core::hotpatch::checked_build<a::B",
            "<frust_core::hotpatch::checked_build<a::B> as h::HotFunction<(u8,), M>>",
            "<frust_core::hotpatch::checked_build<a::B> as h::HotFunction<u8, M>>::call_it",
            "<frust_core::hotpatch::checked_build<> as h::HotFunction<(u8,), M>>::call_it",
        ] {
            let detail = unsupported(parse_seam(odd));
            assert!(detail.contains("shape"), "{odd}: {detail}");
        }
    }

    #[test]
    fn a_real_v0_symbol_is_read_with_or_without_the_mach_o_underscore() {
        for symbol in [MANGLED.to_string(), format!("_{MANGLED}")] {
            let set = SeamSet::from_symbols([symbol.as_str(), "_RNvCs1234_3foo3bar"]).unwrap();
            let instance = set.get(&app("HomePage")).expect("HomePage seam");
            assert_eq!(
                instance.args,
                format!(
                    "(&{}, &mut {}, frust_core::hotpatch::SeamWitness)",
                    app("HomePage"),
                    app("HomeState")
                )
            );
            assert!(set.contains_symbol(&symbol));
            assert_eq!(set.len(), 1);
        }
    }

    #[test]
    fn a_legacy_mangled_seam_is_builder_unsupported() {
        let ident = "_$LT$frust_core..hotpatch..checked_build$LT$a..B$GT$$u20$as$u20$h..HotFunction$LT$$LP$$RF$a..B$C$$RP$$C$M$GT$$GT$";
        let legacy = format!("_ZN{}{ident}7call_it17h0123456789abcdefE", ident.len());
        let detail = unsupported(SeamSet::from_symbols([legacy.as_str()]));
        assert!(detail.contains("v0"), "{detail}");
    }

    // Comparison.

    #[test]
    fn check_reports_a_changed_tuple_and_passes_new_components() {
        let accepted = set(&[("a::Home", "(&a::Home, &mut a::S)")]);
        let candidate = set(&[
            ("a::Home", "(&a::Home, &mut (u32, u32))"),
            ("a::New", "(&a::New, &mut a::T)"),
        ]);
        let report = check(candidate.clone(), &accepted);
        assert_eq!(
            report.changed,
            vec![StateTypeChanged {
                component: "a::Home".into(),
                accepted_args: "(&a::Home, &mut a::S)".into(),
                candidate_args: "(&a::Home, &mut (u32, u32))".into(),
            }]
        );
        assert_eq!(report.present, candidate);
    }

    #[test]
    fn merge_adds_components_and_keeps_accepted_tuples() {
        let mut accepted = set(&[("a::Home", "(1)")]);
        merge(
            &mut accepted,
            &set(&[("a::Home", "(2)"), ("a::New", "(3)")]),
        );
        assert_eq!(accepted.get("a::Home").unwrap().args, "(1)");
        assert_eq!(accepted.get("a::New").unwrap().args, "(3)");
    }

    /// Tests over the fixture workspace, compiled with the pinned toolchain.
    /// Not on Windows: the fixture's objects there are not Mach-O or ELF.
    #[cfg(not(windows))]
    mod fixtures {
        use super::super::super::layout::fixture;
        use super::super::*;
        use super::app;

        fn seams(rlib: PathBuf) -> SeamSet {
            SeamSet::from_inputs(&[rlib]).expect("reading the fixture's seams")
        }

        #[test]
        fn the_fat_build_has_one_seam_instance_per_mounted_component() {
            let base = seams(fixture::base());
            assert_eq!(
                base.instances.keys().cloned().collect::<Vec<_>>(),
                vec![app("Counter"), app("HomePage")]
            );
            let home = base.get(&app("HomePage")).unwrap();
            assert!(
                home.args.contains(&format!("&mut {}", app("HomeState"))),
                "{}",
                home.args
            );
            eprintln!("seam symbols: {:?}", home.symbols);
        }

        #[test]
        fn a_component_added_in_patch_1_whose_state_identity_changes_in_patch_2_is_refused() {
            let mut accepted = seams(fixture::base());
            let patch1 = check(seams(fixture::edited("settings-p1")), &accepted);
            assert_eq!(patch1.changed, vec![]);
            assert!(patch1.present.get(&app("Settings")).is_some());
            merge(&mut accepted, &patch1.present);

            let patch2 = seams(fixture::edited("settings-p2"));
            assert_eq!(
                check(patch2.clone(), &seams(fixture::base())).changed,
                vec![],
                "the base alone has no Settings instance, so it cannot catch this"
            );
            let report = check(patch2, &accepted);
            assert_eq!(report.changed.len(), 1, "{:?}", report.changed);
            let changed = &report.changed[0];
            assert_eq!(changed.component, app("Settings"));
            assert!(changed.accepted_args.contains(&app("SettingsState")));
            assert!(
                changed.candidate_args.contains("&mut (bool, u8)"),
                "{}",
                changed.candidate_args
            );
            assert!(report.present.get(&app("Settings")).is_some());
            assert!(report.present.get(&app("HomePage")).is_some());
        }

        /// Row D2 keeps the State type's path, so the seam symbol is unchanged:
        /// only the layout gate can refuse it.
        #[test]
        fn a_state_field_add_keeps_the_seam_identity() {
            let report = check(
                seams(fixture::edited("d2-field-add")),
                &seams(fixture::base()),
            );
            assert_eq!(report.changed, vec![]);
        }
    }
}
