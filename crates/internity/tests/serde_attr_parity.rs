// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parity tests asserting that the serde field-schema attributes honored by the
//! `DeserializeIn` derive accept exactly the same wire format as an equivalent
//! `serde::Deserialize` derive. Each case pairs an interned struct (`Sym` fields)
//! with a plain serde struct (`String` fields) carrying identical attributes and
//! checks that both accept/reject the same JSON and agree on the decoded strings.

#![cfg(feature = "serde")]

use internity::de::DeserializeIn;
use internity::{LocalLexicon, Sym};
use serde::Deserialize;

fn de_in<T: for<'de> DeserializeIn<'de, LocalLexicon>>(lex: &mut LocalLexicon, json: &str) -> Result<T, serde_json::Error> {
    T::deserialize_in(lex, &mut serde_json::Deserializer::from_str(json))
}

fn de<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(json)
}

// ----- rename + rename_all -------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenamePlain {
    #[serde(rename = "id")]
    identifier: String,
    long_name: String,
}

#[derive(DeserializeIn)]
#[serde(rename_all = "camelCase")]
struct RenameIn {
    #[serde(rename = "id")]
    identifier: Sym,
    long_name: Sym,
}

#[test]
fn rename_and_rename_all_match_serde() {
    let good = r#"{"id":"x","longName":"hello world"}"#;
    // Wrong (unrenamed) keys must fail for both.
    let bad = r#"{"identifier":"x","long_name":"hello world"}"#;

    let plain: RenamePlain = de(good).unwrap();
    let mut lex = LocalLexicon::new();
    let interned: RenameIn = de_in(&mut lex, good).unwrap();
    assert_eq!(plain.identifier, lex.resolve(interned.identifier));
    assert_eq!(plain.long_name, lex.resolve(interned.long_name));

    assert!(de::<RenamePlain>(bad).is_err());
    let mut lex = LocalLexicon::new();
    assert!(de_in::<RenameIn>(&mut lex, bad).is_err());
}

// ----- alias ----------------------------------------------------------------

#[derive(Deserialize)]
struct AliasPlain {
    #[serde(alias = "surname", alias = "family_name")]
    last: String,
}

#[derive(DeserializeIn)]
struct AliasIn {
    #[serde(alias = "surname", alias = "family_name")]
    last: Sym,
}

#[test]
fn aliases_match_serde() {
    for json in [r#"{"last":"a"}"#, r#"{"surname":"a"}"#, r#"{"family_name":"a"}"#] {
        let plain: AliasPlain = de(json).unwrap();
        let mut lex = LocalLexicon::new();
        let interned: AliasIn = de_in(&mut lex, json).unwrap();
        assert_eq!(plain.last, lex.resolve(interned.last));
    }
}

// ----- skip + field default -------------------------------------------------

fn seven() -> u32 {
    7
}

#[derive(Deserialize)]
struct SkipPlain {
    name: String,
    #[serde(skip)]
    ignored: u32,
    #[serde(default = "seven")]
    weight: u32,
}

#[derive(DeserializeIn)]
struct SkipIn {
    name: Sym,
    #[serde(skip)]
    ignored: u32,
    #[serde(default = "seven")]
    weight: u32,
}

#[test]
fn skip_and_field_default_match_serde() {
    // `weight` omitted -> its default; `ignored` never read.
    let json = r#"{"name":"n"}"#;
    let plain: SkipPlain = de(json).unwrap();
    assert_eq!(plain.name, "n");
    assert_eq!((plain.ignored, plain.weight), (0, 7));

    let mut lex = LocalLexicon::new();
    let interned: SkipIn = de_in(&mut lex, json).unwrap();
    assert_eq!(lex.resolve(interned.name), "n");
    assert_eq!((interned.ignored, interned.weight), (0, 7));

    // Providing `weight` overrides the default; a `ignored` key is rejected by
    // neither (unknown keys are ignored without deny_unknown_fields).
    let json = r#"{"name":"n","weight":3,"ignored":99}"#;
    let plain: SkipPlain = de(json).unwrap();
    let mut lex = LocalLexicon::new();
    let interned: SkipIn = de_in(&mut lex, json).unwrap();
    assert_eq!(plain.weight, interned.weight);
    assert_eq!((plain.ignored, interned.ignored), (0, 0));
}

// ----- container default ----------------------------------------------------

#[derive(Deserialize)]
#[serde(default)]
struct ContainerDefaultPlain {
    a: String,
    b: u32,
}

impl Default for ContainerDefaultPlain {
    fn default() -> Self {
        Self { a: String::new(), b: 42 }
    }
}

#[derive(DeserializeIn)]
#[serde(default)]
struct ContainerDefaultIn {
    #[internity(via_serde)]
    a: String,
    b: u32,
}

impl Default for ContainerDefaultIn {
    fn default() -> Self {
        Self { a: String::new(), b: 42 }
    }
}

#[test]
fn container_default_fills_missing_fields() {
    // Only `b` present -> `a` from container default.
    let json = r#"{"b":5}"#;
    let plain: ContainerDefaultPlain = de(json).unwrap();
    assert_eq!((plain.a.as_str(), plain.b), ("", 5));

    let mut lex = LocalLexicon::new();
    let interned: ContainerDefaultIn = de_in(&mut lex, json).unwrap();
    assert_eq!((interned.a.as_str(), interned.b), ("", 5));

    // Only `a` present -> `b` from container default (42).
    let json = r#"{"a":"x"}"#;
    let plain: ContainerDefaultPlain = de(json).unwrap();
    let mut lex = LocalLexicon::new();
    let interned: ContainerDefaultIn = de_in(&mut lex, json).unwrap();
    assert_eq!((plain.a.as_str(), plain.b), ("x", 42));
    assert_eq!((interned.a.as_str(), interned.b), ("x", 42));

    // Empty map -> both from container default.
    let mut lex = LocalLexicon::new();
    let interned: ContainerDefaultIn = de_in(&mut lex, "{}").unwrap();
    assert_eq!((interned.a.as_str(), interned.b), ("", 42));
}

// ----- deny_unknown_fields --------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DenyPlain {
    a: String,
}

#[derive(DeserializeIn)]
#[serde(deny_unknown_fields)]
struct DenyIn {
    a: Sym,
}

#[test]
fn deny_unknown_fields_matches_serde() {
    let ok = r#"{"a":"x"}"#;
    let bad = r#"{"a":"x","b":1}"#;

    let plain: DenyPlain = de(ok).unwrap();
    assert_eq!(plain.a, "x");
    assert!(de::<DenyPlain>(bad).is_err());

    let mut lex = LocalLexicon::new();
    let interned: DenyIn = de_in(&mut lex, ok).unwrap();
    assert_eq!(lex.resolve(interned.a), "x");
    let mut lex = LocalLexicon::new();
    assert!(de_in::<DenyIn>(&mut lex, bad).is_err());
}

// ----- transparent ----------------------------------------------------------

#[derive(Deserialize)]
#[serde(transparent)]
struct TransparentPlain {
    inner: String,
}

#[derive(DeserializeIn)]
#[serde(transparent)]
struct TransparentIn {
    inner: Sym,
}

#[test]
fn transparent_matches_serde() {
    let json = r#""bare""#;
    let plain: TransparentPlain = de(json).unwrap();
    assert_eq!(plain.inner, "bare");

    let mut lex = LocalLexicon::new();
    let interned: TransparentIn = de_in(&mut lex, json).unwrap();
    assert_eq!(lex.resolve(interned.inner), "bare");
}

// ----- with / deserialize_with ----------------------------------------------

fn shout<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(String::deserialize(d)?.to_uppercase())
}

#[derive(Deserialize)]
struct WithPlain {
    #[serde(deserialize_with = "shout")]
    tag: String,
}

#[derive(DeserializeIn)]
struct WithIn {
    #[serde(deserialize_with = "shout")]
    tag: String,
    key: Sym,
}

#[derive(Deserialize)]
struct WithPlain2 {
    #[serde(deserialize_with = "shout")]
    tag: String,
    key: String,
}

#[test]
fn deserialize_with_matches_serde() {
    let single = r#"{"tag":"hi"}"#;
    let plain: WithPlain = de(single).unwrap();
    assert_eq!(plain.tag, "HI");

    let json = r#"{"tag":"hi","key":"k"}"#;
    let plain: WithPlain2 = de(json).unwrap();
    let mut lex = LocalLexicon::new();
    let interned: WithIn = de_in(&mut lex, json).unwrap();
    assert_eq!(plain.tag, interned.tag);
    assert_eq!(plain.key, lex.resolve(interned.key));
    assert_eq!(interned.tag, "HI");
    assert_eq!(lex.resolve(interned.key), "k");
}

// Ensure a generated helper name cannot shadow the user's `deserialize_with`
// function.
#[expect(non_snake_case, reason = "deliberately collides with a generated helper name")]
fn __InternityWithSeed0ForCollide<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(String::deserialize(d)?.to_uppercase())
}

#[derive(DeserializeIn)]
struct Collide {
    #[serde(deserialize_with = "__InternityWithSeed0ForCollide")]
    tag: String,
    key: Sym,
}

#[test]
fn deserialize_with_colliding_helper_name_resolves_to_user_function() {
    let json = r#"{"tag":"hi","key":"k"}"#;
    let mut lex = LocalLexicon::new();
    let interned: Collide = de_in(&mut lex, json).unwrap();
    assert_eq!(interned.tag, "HI");
    assert_eq!(lex.resolve(interned.key), "k");
}
