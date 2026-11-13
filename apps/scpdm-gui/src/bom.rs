//! An assembly's bill of materials, counted as SolidCraft counts it: every placement
//! (occurrence) of a component is one piece, through components nested in the design and through
//! linked designs (a linked assembly brings its own parts along, times its quantity). Each
//! component's bill of materials type counts: a phantom has no row and what it holds counts in its
//! parent; a reference counts nowhere; a purchased component is one row with nothing inside it
//! counted; an inseparable one is one row and of what it holds only purchased parts count.
//!
//! Rows are definitions: a linked design (its file), a standard part (its catalogue size), or a
//! component defined in a design file. A linked design's row shows that design's own properties
//! (the vault's revision), its component's where the file can't be read.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::Path;

use scformat::{BomType, Component, DesignInfo};

/// What a row of the bill of materials is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Definition {
    /// A linked design, by vault path (or the stored path, when it leads out of the vault).
    Design(String),
    /// A standard part: by its part number (SolidCraft gives one, `ISO 4762 M6x20`), else its
    /// catalogue size.
    Standard(String),
    /// A component defined in a design file: the file's vault path and the component's id.
    Component(String, u64),
}

/// One row: a definition and how many there are in all.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub definition: Definition,
    /// The vault path to show and select: the linked design, or the file defining the component.
    pub path: String,
    pub name: String,
    pub part_number: String,
    pub description: String,
    pub revision: String,
    pub bom_type: BomType,
    pub quantity: u32,
    /// Whether it holds placed components itself (its parts are listed too).
    pub assembly: bool,
    /// A linked design whose file is missing or not a readable design.
    pub missing: bool,
}

/// How deep assemblies nest at most (a design linking to itself, through others, stops there).
const MAX_DEPTH: usize = 32;
/// The most placements one bill of materials walks.
const MAX_PLACEMENTS: usize = 100_000;

/// What a walk below a component lists.
#[derive(Clone, Copy, PartialEq)]
enum Walk {
    Listed,
    /// Inside an inseparable component: only its purchased parts.
    Purchased,
}

/// The bill of materials of the design at vault path `root`. `design_of` reads a design by vault
/// path (`None`: missing or unreadable); `resolve` turns a link stored in a design into a vault path
/// (`None`: it leads out of the vault).
pub fn lines<'a>(root: &str, design_of: impl Fn(&str) -> Option<&'a DesignInfo>, resolve: impl Fn(&str, &str) -> Option<String>) -> Vec<Line> {
    let mut b = Builder { design_of: &design_of, resolve: &resolve, rows: BTreeMap::new(), files: vec![root.to_owned()], placed: 0, designs: PhantomData };
    if let Some(info) = design_of(root) {
        b.walk(root, info, 0, 1, Walk::Listed);
    }
    let mut out: Vec<Line> = b.rows.into_values().collect();
    // By part number; rows without one after them, by name.
    out.sort_by(|a, b| (a.part_number.is_empty(), &a.part_number, &a.name, &a.definition).cmp(&(b.part_number.is_empty(), &b.part_number, &b.name, &b.definition)));
    out
}

struct Builder<'f, 'a, D, R> {
    design_of: &'f D,
    resolve: &'f R,
    rows: BTreeMap<Definition, Line>,
    /// The design files being walked, outermost first (a file met again is a loop).
    files: Vec<String>,
    placed: usize,
    designs: PhantomData<&'a DesignInfo>,
}

impl<'a, D: Fn(&str) -> Option<&'a DesignInfo>, R: Fn(&str, &str) -> Option<String>> Builder<'_, 'a, D, R> {
    /// Counts what `parent` (a component of `file`, 0 its root) places, `times` over.
    fn walk(&mut self, file: &str, info: &'a DesignInfo, parent: u64, times: u32, walk: Walk) {
        for o in info.occurrences.iter().filter(|o| o.parent == parent) {
            self.placed += 1;
            if self.placed > MAX_PLACEMENTS {
                return;
            }
            let Some(c) = info.components.iter().find(|c| c.id == o.component) else { continue };
            // A component placed inside itself (a damaged file) is counted once, not followed.
            let nested = c.id != parent;
            match (c.metadata.bom_type(), walk) {
                (BomType::Reference, _) => {}
                (BomType::Phantom, _) => {
                    if nested {
                        self.walk(file, info, c.id, times, walk);
                    }
                }
                (BomType::Purchased, _) => self.add(file, info, c, times),
                (_, Walk::Purchased) => {
                    if nested {
                        self.inside(file, info, c, times, Walk::Purchased);
                    }
                }
                (BomType::Inseparable, Walk::Listed) => {
                    self.add(file, info, c, times);
                    if nested {
                        self.inside(file, info, c, times, Walk::Purchased);
                    }
                }
                (BomType::Normal, Walk::Listed) => {
                    self.add(file, info, c, times);
                    if nested {
                        self.inside(file, info, c, times, Walk::Listed);
                    }
                }
            }
        }
    }

    /// What component `c` of `file` holds: components placed in it there, and a linked design's.
    fn inside(&mut self, file: &str, info: &'a DesignInfo, c: &Component, times: u32, walk: Walk) {
        self.walk(file, info, c.id, times, walk);
        if let Some((path, Some(linked))) = self.linked(file, info, c.id) {
            if self.files.len() < MAX_DEPTH && !self.files.contains(&path) {
                self.files.push(path.clone());
                self.walk(&path, linked, 0, times, walk);
                self.files.pop();
            }
        }
    }

    /// The design component `id` of `file` links to: its vault path, and the design if readable.
    fn linked(&self, file: &str, info: &DesignInfo, id: u64) -> Option<(String, Option<&'a DesignInfo>)> {
        let link = info.links.iter().find(|l| l.component == id)?;
        match (self.resolve)(file, &link.path) {
            Some(path) => {
                let design = (self.design_of)(&path);
                Some((path, design))
            }
            None => Some((link.path.clone(), None)),
        }
    }

    fn add(&mut self, file: &str, info: &'a DesignInfo, c: &Component, times: u32) {
        let holds = |d: &DesignInfo, id: u64| d.occurrences.iter().any(|o| o.parent == id && o.component != id);
        let own = &c.metadata;
        let text = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or_default().to_owned();
        let mut line = Line {
            definition: Definition::Component(file.to_owned(), c.id),
            path: file.to_owned(),
            name: c.name.clone(),
            part_number: text(&own.part_number),
            description: text(&own.description),
            revision: text(&own.revision),
            bom_type: own.bom_type(),
            quantity: 0,
            assembly: holds(info, c.id),
            missing: false,
        };
        if let Some((path, design)) = self.linked(file, info, c.id) {
            line.definition = Definition::Design(path.clone());
            line.path = path.clone();
            match design {
                Some(d) => {
                    let m = &d.metadata;
                    let stem = path.rsplit_once('/').map_or(path.as_str(), |(_, n)| n);
                    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
                    line.name = d.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| stem.to_owned());
                    for (slot, value) in [(&mut line.part_number, &m.part_number), (&mut line.description, &m.description), (&mut line.revision, &m.revision)] {
                        if value.as_deref().is_some_and(|v| !v.trim().is_empty()) {
                            *slot = text(value);
                        }
                    }
                    line.assembly |= holds(d, 0);
                }
                None => line.missing = true,
            }
        } else if let Some(part) = info.standard_parts.get(&c.id) {
            let key = if line.part_number.is_empty() { part.label() } else { line.part_number.clone() };
            line.definition = Definition::Standard(key);
            if line.name.trim().is_empty() {
                line.name = part.label();
            }
        }
        let row = self.rows.entry(line.definition.clone()).or_insert(line);
        row.quantity = row.quantity.saturating_add(times);
    }
}

/// Every design `root` uses, directly or through others, without `root`: every linked design,
/// whatever its bill of materials type (opening the assembly needs them all). `links_of` gives a
/// design's links as vault paths, `None` when it cannot be read.
pub fn used(root: &str, links_of: impl Fn(&str) -> Option<Vec<String>>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut stack = vec![(root.to_owned(), 0usize)];
    while let Some((design, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            continue;
        }
        for target in links_of(&design).unwrap_or_default() {
            if target != root && !seen.contains(&target) {
                seen.push(target.clone());
                stack.push((target, depth + 1));
            }
        }
    }
    seen.sort();
    seen
}

/// One field of a CSV file: quoted when it needs to be.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r', ';']) || text.starts_with(' ') || text.ends_with(' ') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// A CSV file (comma separated, a header row, CRLF line ends as spreadsheets expect).
pub fn csv(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    let mut row = |cells: Vec<String>| {
        out.push_str(&cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    };
    row(header.iter().map(|h| (*h).to_owned()).collect());
    for r in rows {
        row(r.clone());
    }
    out
}

/// Writes `designs` (vault paths) into a zip at `out`, below a folder `name`, keeping their
/// folders so relative links work where it is unpacked; the designs that were missing.
pub fn pack(root: &Path, name: &str, designs: &[String], out: &Path) -> Result<Vec<String>, String> {
    use std::io::Write;
    let tmp = out.with_extension("zip.partial");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut missing = Vec::new();
    let written = (|| -> Result<(), String> {
        for design in designs {
            let Ok(bytes) = std::fs::read(root.join(design)) else {
                missing.push(design.clone());
                continue;
            };
            zip.start_file(format!("{name}/{design}"), options).map_err(|e| e.to_string())?;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    })();
    match written.and_then(|()| std::fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))) {
        Ok(()) => Ok(missing),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn links(map: &[(&str, &[&str])]) -> impl Fn(&str) -> Option<Vec<String>> {
        let map: HashMap<String, Vec<String>> = map.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect();
        move |p: &str| map.get(p).cloned()
    }

    fn design(doc: serde_json::Value) -> DesignInfo {
        scformat::read(std::io::Cursor::new(serde_json::to_vec(&doc).unwrap())).unwrap()
    }

    /// Designs by name, with links stored as the target's name.
    fn bom(root: &str, designs: &[(&str, serde_json::Value)]) -> Vec<(String, u32, bool, bool)> {
        let designs: HashMap<String, DesignInfo> = designs.iter().map(|(k, v)| ((*k).to_owned(), design(v.clone()))).collect();
        lines(root, |p| designs.get(p), |_, l| Some(l.to_owned())).into_iter().map(|l| (l.name, l.quantity, l.assembly, l.missing)).collect()
    }

    fn part(name: &str) -> serde_json::Value {
        json!({"format": "solidcraft/55", "name": name})
    }

    #[test]
    fn counts_every_placement_through_linked_sub_assemblies() {
        // A box: a lid, two hinges (each with two screws and a pin), four screws of its own. The
        // screws are one linked component placed four times, as Insert Design's reuse leaves them.
        let hinge = json!({
            "name": "Hinge",
            "components": [{"id": 1, "name": "Screw"}, {"id": 2, "name": "Pin"}],
            "occurrences": [{"id": 3, "component": 1}, {"id": 4, "component": 1}, {"id": 5, "component": 2}],
            "links": {"1": {"path": "screw", "feature": 1}, "2": {"path": "pin", "feature": 2}},
        });
        let boxed = json!({
            "name": "Box",
            "components": [{"id": 1, "name": "Lid"}, {"id": 2, "name": "Hinge"}, {"id": 3, "name": "Screw"}],
            "occurrences": [
                {"id": 4, "component": 1}, {"id": 5, "component": 2}, {"id": 6, "component": 2},
                {"id": 7, "component": 3}, {"id": 8, "component": 3}, {"id": 9, "component": 3}, {"id": 10, "component": 3},
            ],
            "links": {"1": {"path": "lid", "feature": 1}, "2": {"path": "hinge", "feature": 2}, "3": {"path": "screw", "feature": 3}},
        });
        let got = bom("box", &[("box", boxed), ("hinge", hinge), ("lid", part("Lid")), ("screw", part("Screw")), ("pin", part("Pin"))]);
        let want = [("Hinge", 2, true, false), ("Lid", 1, false, false), ("Pin", 2, false, false), ("Screw", 8, false, false)];
        assert_eq!(got, want.map(|(n, q, a, m)| (n.to_owned(), q, a, m)));
    }

    #[test]
    fn bom_types_count_as_in_solidcraft() {
        // Frame (inseparable) holds a made bracket and a bought bolt; a phantom group holds two
        // nuts; a reference jig holds a bolt; a bought motor holds a rotor.
        let doc = json!({
            "name": "Machine",
            "components": [
                {"id": 1, "name": "Frame", "metadata": {"bom_type": "inseparable", "part_number": "FR-1"}},
                {"id": 2, "name": "Bracket", "parent": 1},
                {"id": 3, "name": "Bolt", "metadata": {"bom_type": "purchased"}},
                {"id": 4, "name": "Group", "metadata": {"bom_type": "phantom"}},
                {"id": 5, "name": "Nut"},
                {"id": 6, "name": "Jig", "metadata": {"bom_type": "reference"}},
                {"id": 7, "name": "Motor", "metadata": {"bom_type": "purchased"}},
                {"id": 8, "name": "Rotor", "parent": 7},
                {"id": 9, "name": "Washer", "metadata": {"bom_type": "kit"}},
            ],
            "occurrences": [
                {"component": 1}, {"component": 2, "parent": 1}, {"component": 3, "parent": 1},
                {"component": 4}, {"component": 5, "parent": 4}, {"component": 5, "parent": 4},
                {"component": 6}, {"component": 3, "parent": 6},
                {"component": 7}, {"component": 8, "parent": 7},
                {"component": 9},
            ],
        });
        let got = bom("m", &[("m", doc)]);
        let want = [("Frame", 1, true), ("Bolt", 1, false), ("Motor", 1, true), ("Nut", 2, false), ("Washer", 1, false)];
        assert_eq!(got, want.map(|(n, q, a)| (n.to_owned(), q, a, false)), "a type this build doesn't know counts as normal");
    }

    #[test]
    fn standard_parts_are_rows_by_part_number_and_old_designs_place_each_component_once() {
        // As SolidCraft 0.41's parts.insert makes them: a part number and a name of their own.
        let screw = json!({"part_number": "ISO 4762 M6x20", "description": "Socket head cap screw M6x20"});
        let doc = json!({
            "name": "Plate",
            "components": [
                {"id": 1, "name": "SHCS M6x20", "metadata": screw}, {"id": 2, "name": "SHCS M6x20 (2)", "metadata": screw},
                {"id": 3, "name": ""}, {"id": 4, "name": "Plate"},
            ],
            "parts": {
                "1": {"family": "socket_head_cap_screw", "size": "M6", "length": 20.0},
                "2": {"family": "socket_head_cap_screw", "size": "M6", "length": 20.0},
                "3": {"family": "hex_nut", "size": "M6"},
            },
        });
        let want = [("SHCS M6x20", 2), ("Plate", 1), ("hex_nut M6", 1)];
        assert_eq!(bom("p", &[("p", doc)]), want.map(|(n, q)| (n.to_owned(), q, false, false)));
    }

    #[test]
    fn linked_rows_show_the_designs_properties_and_missing_designs_and_loops_are_shown_not_followed() {
        let a = json!({
            "name": "A",
            "components": [{"id": 1, "name": "B", "metadata": {"part_number": "OLD-1", "revision": "A"}}, {"id": 2, "name": "Gone"}],
            "occurrences": [{"component": 1}, {"component": 2}],
            "links": {"1": {"path": "b", "feature": 1}, "2": {"path": "gone", "feature": 2}},
        });
        let b = json!({
            "name": "B", "metadata": {"part_number": "B-1", "revision": "C"},
            "components": [{"id": 1, "name": "A"}], "occurrences": [{"component": 1}], "links": {"1": {"path": "a", "feature": 1}},
        });
        let designs: HashMap<String, DesignInfo> = [("a", a), ("b", b)].into_iter().map(|(k, v)| (k.to_owned(), design(v))).collect();
        let got = lines("a", |p| designs.get(p), |_, l| Some(l.to_owned()));
        let got: Vec<(&str, &str, &str, u32, bool)> = got.iter().map(|l| (l.path.as_str(), l.part_number.as_str(), l.revision.as_str(), l.quantity, l.missing)).collect();
        assert_eq!(got, [("b", "B-1", "C", 1, false), ("a", "", "", 1, false), ("gone", "", "", 1, true)]);
    }

    /// The assembly SolidCraft 0.41 saved (scformat's `tests/fixtures/sc041`). SolidCraft's own
    /// `assembly.bom` lists the plate twice, the pair once and the screw once; the PDM follows the
    /// pair into its file too, where the bracket is placed twice.
    #[test]
    fn counts_an_assembly_solidcraft_041_saved() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/scformat/tests/fixtures/sc041");
        let designs: HashMap<String, DesignInfo> = ["parts/plate.sc", "parts/bracket.sc", "asm/pair.sc", "asm/bench.sc"]
            .into_iter()
            .map(|p| (p.to_owned(), scformat::read_file(root.join(p)).unwrap()))
            .collect();
        let resolve = |from: &str, link: &str| {
            let mut parts: Vec<&str> = from.split('/').collect();
            parts.pop();
            for part in link.split('/') {
                match part {
                    ".." => drop(parts.pop()),
                    p => parts.push(p),
                }
            }
            Some(parts.join("/"))
        };
        let got: Vec<(String, String, u32, bool)> =
            lines("asm/bench.sc", |p| designs.get(p), resolve).into_iter().map(|l| (l.part_number, l.name, l.quantity, l.assembly)).collect();
        let want = [("BR-200", "bracket", 2, false), ("ISO 4762 M6x20", "SHCS M6x20", 1, false), ("PL-100", "plate", 2, false), ("SUB-300", "pair", 1, true)];
        assert_eq!(got, want.map(|(pn, n, q, a)| (pn.to_owned(), n.to_owned(), q, a)));
    }

    #[test]
    fn used_lists_every_linked_design_once_and_stops_at_loops() {
        assert_eq!(used("a", links(&[("a", &["b", "gone"]), ("b", &["a", "c"]), ("c", &[])])), ["b", "c", "gone"]);
    }

    #[test]
    fn packs_designs_in_their_folders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("asm")).unwrap();
        std::fs::create_dir_all(dir.path().join("parts")).unwrap();
        std::fs::write(dir.path().join("asm/box.solidcraft"), "box").unwrap();
        std::fs::write(dir.path().join("parts/lid.solidcraft"), "lid").unwrap();
        let out = dir.path().join("box.zip");
        let designs = ["asm/box.solidcraft".to_owned(), "parts/lid.solidcraft".to_owned(), "parts/gone.solidcraft".to_owned()];
        assert_eq!(pack(dir.path(), "box", &designs, &out).unwrap(), ["parts/gone.solidcraft"]);
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_owned()).collect();
        assert_eq!(names, ["box/asm/box.solidcraft", "box/parts/lid.solidcraft"]);
        let mut lid = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("box/parts/lid.solidcraft").unwrap(), &mut lid).unwrap();
        assert_eq!(lid, "lid");
        assert!(!dir.path().join("box.zip.partial").exists());
    }

    #[test]
    fn writes_csv_that_spreadsheets_read() {
        let text = csv(&["Part number", "Description"], &[vec!["P-1".into(), "Lid, \"deep\"".into()], vec![" P-2".into(), String::new()]]);
        assert_eq!(text, "Part number,Description\r\nP-1,\"Lid, \"\"deep\"\"\"\r\n\" P-2\",\r\n");
    }
}
