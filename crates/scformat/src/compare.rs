//! What changed between two versions of a design: properties, parameters, features, components,
//! links, and which other parts of the document differ.

use std::collections::BTreeMap;

use serde_json::Value;

/// A value that was added (`before` absent), removed (`after` absent) or changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub what: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A feature of the timeline, matched by its id across the two versions.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureChange {
    pub id: u64,
    /// The name in the newer version (or the older one, for a removed feature).
    pub name: String,
    /// `sketch`, `extrude`, `fillet`…
    pub kind: String,
    pub change: FeatureChangeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FeatureChangeKind {
    Added,
    Removed,
    /// Renamed, suppressed or unsuppressed, and/or its settings changed (their keys).
    Edited { renamed_from: Option<String>, suppressed: Option<bool>, settings: Vec<String> },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    /// Name, units, format and the design properties.
    pub properties: Vec<Change>,
    /// By parameter name: the expression (with its unit).
    pub parameters: Vec<Change>,
    pub features: Vec<FeatureChange>,
    /// Features in another order (besides additions and removals).
    pub features_reordered: bool,
    /// By component: its name.
    pub components: Vec<Change>,
    /// By component: the linked design's path.
    pub links: Vec<Change>,
    /// Other parts of the document that differ (`appearances`, `materials`, `configs`…).
    pub other: Vec<String>,
}

impl Comparison {
    pub fn is_empty(&self) -> bool {
        *self == Comparison::default()
    }
}

/// Compares two design files (containers or plain JSON).
pub fn compare(before: &[u8], after: &[u8]) -> crate::Result<Comparison> {
    Ok(compare_documents(&crate::read_document(before)?, &crate::read_document(after)?))
}

/// Keys handled by name; everything else is reported in `other` when it differs.
const COMPARED: [&str; 9] = ["format", "name", "units", "metadata", "params", "features", "components", "links", "next_id"];

pub fn compare_documents(a: &Value, b: &Value) -> Comparison {
    let mut c = Comparison::default();

    for key in ["name", "units", "format"] {
        push_change(&mut c.properties, key, text(&a[key]), text(&b[key]));
    }
    let (ma, mb) = (object(&a["metadata"]), object(&b["metadata"]));
    for key in union_keys(&ma, &mb) {
        push_change(&mut c.properties, &key, ma.get(&key).and_then(|v| text(v)), mb.get(&key).and_then(|v| text(v)));
    }

    let params = |doc: &Value| -> BTreeMap<String, String> {
        array(&doc["params"])
            .iter()
            .filter_map(|p| {
                let name = p["name"].as_str()?.to_owned();
                let expr = text(&p["expr"]).or_else(|| text(&p["value"])).unwrap_or_default();
                Some((name, expr))
            })
            .collect()
    };
    let (pa, pb) = (params(a), params(b));
    for name in union_keys(&pa, &pb) {
        push_change(&mut c.parameters, &name, pa.get(&name).cloned(), pb.get(&name).cloned());
    }

    compare_features(a, b, &mut c);

    let components = |doc: &Value| -> BTreeMap<u64, String> {
        array(&doc["components"])
            .iter()
            .filter_map(|comp| Some((comp["id"].as_u64()?, comp["name"].as_str().unwrap_or("").to_owned())))
            .collect()
    };
    let (ca, cb) = (components(a), components(b));
    for id in union_keys(&ca, &cb) {
        let label = cb.get(&id).or(ca.get(&id)).cloned().unwrap_or_else(|| format!("component {id}"));
        push_change(&mut c.components, &label, ca.get(&id).cloned(), cb.get(&id).cloned());
    }
    let links = |doc: &Value| -> BTreeMap<String, String> {
        object(&doc["links"]).into_iter().filter_map(|(id, l)| Some((id, l["path"].as_str()?.to_owned()))).collect()
    };
    let (la, lb) = (links(a), links(b));
    for id in union_keys(&la, &lb) {
        let label = id
            .parse()
            .ok()
            .and_then(|n: u64| cb.get(&n).or(ca.get(&n)).cloned())
            .unwrap_or_else(|| format!("component {id}"));
        push_change(&mut c.links, &label, la.get(&id).cloned(), lb.get(&id).cloned());
    }

    let (oa, ob) = (object(a), object(b));
    for key in union_keys(&oa, &ob) {
        if !COMPARED.contains(&key.as_str()) && oa.get(&key) != ob.get(&key) {
            c.other.push(key);
        }
    }
    c
}

fn compare_features(a: &Value, b: &Value, c: &mut Comparison) {
    let index = |doc: &Value| -> Vec<(u64, Value)> {
        array(&doc["features"]).iter().filter_map(|f| Some((f["id"].as_u64()?, f.clone()))).collect()
    };
    let (fa, fb) = (index(a), index(b));
    let (ma, mb): (BTreeMap<u64, &Value>, BTreeMap<u64, &Value>) =
        (fa.iter().map(|(i, f)| (*i, f)).collect(), fb.iter().map(|(i, f)| (*i, f)).collect());
    let name = |f: &Value| f["name"].as_str().unwrap_or("").to_owned();
    let kind = |f: &Value| f["type"].as_str().unwrap_or("feature").to_owned();

    // In the newer version's order, then what was removed.
    for (id, after) in &fb {
        let change = match ma.get(id) {
            None => FeatureChangeKind::Added,
            Some(before) => {
                let renamed_from = (name(before) != name(after)).then(|| name(before));
                let suppressed = (before["suppressed"] != after["suppressed"]).then(|| after["suppressed"].as_bool() == Some(true));
                let keys = union_keys(&object(before), &object(after));
                let settings: Vec<String> = keys
                    .into_iter()
                    .filter(|k| !["id", "name", "suppressed"].contains(&k.as_str()) && before.get(k) != after.get(k))
                    .collect();
                if renamed_from.is_none() && suppressed.is_none() && settings.is_empty() {
                    continue;
                }
                FeatureChangeKind::Edited { renamed_from, suppressed, settings }
            }
        };
        c.features.push(FeatureChange { id: *id, name: name(after), kind: kind(after), change });
    }
    for (id, before) in &fa {
        if !mb.contains_key(id) {
            c.features.push(FeatureChange { id: *id, name: name(before), kind: kind(before), change: FeatureChangeKind::Removed });
        }
    }
    let kept_a: Vec<u64> = fa.iter().map(|(i, _)| *i).filter(|i| mb.contains_key(i)).collect();
    let kept_b: Vec<u64> = fb.iter().map(|(i, _)| *i).filter(|i| ma.contains_key(i)).collect();
    c.features_reordered = kept_a != kept_b;
}

fn push_change(out: &mut Vec<Change>, what: &str, before: Option<String>, after: Option<String>) {
    if before != after {
        out.push(Change { what: what.to_owned(), before, after });
    }
}

/// A value as text: strings as they are, anything else as compact JSON; absent/null is `None`.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn object(v: &Value) -> BTreeMap<String, Value> {
    v.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn union_keys<K: Ord + Clone, V>(a: &BTreeMap<K, V>, b: &BTreeMap<K, V>) -> Vec<K> {
    let mut keys: Vec<K> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys
}
