//! An assembly's bill of materials, from its links: every linked component is one piece of the
//! design it links to, and a linked assembly brings its own parts along (times its quantity).

use std::collections::BTreeMap;
use std::path::Path;

/// One design in an assembly, with how many there are in all.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Vault path.
    pub path: String,
    pub quantity: u32,
    /// Whether it links to designs itself.
    pub assembly: bool,
    /// The file is missing or not a readable design.
    pub missing: bool,
}

/// How deep assemblies nest at most (a design linking to itself, through others, stops there).
const MAX_DEPTH: usize = 32;

/// The designs `root` uses, with their total quantities, by path. `links_of` gives a design's
/// links as vault paths (one per linked component), `None` when it cannot be read.
pub fn lines(root: &str, links_of: impl Fn(&str) -> Option<Vec<String>>) -> Vec<Line> {
    let mut totals: BTreeMap<String, Line> = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    add(root, 1, &links_of, &mut totals, &mut stack);
    totals.into_values().collect()
}

fn add(design: &str, times: u32, links_of: &impl Fn(&str) -> Option<Vec<String>>, totals: &mut BTreeMap<String, Line>, stack: &mut Vec<String>) {
    let Some(links) = links_of(design) else { return };
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for target in links {
        *counts.entry(target).or_default() += 1;
    }
    for (target, count) in counts {
        let quantity = times.saturating_mul(count);
        let theirs = links_of(&target);
        let line = totals.entry(target.clone()).or_insert_with(|| Line {
            path: target.clone(),
            quantity: 0,
            assembly: theirs.as_ref().is_some_and(|l| !l.is_empty()),
            missing: theirs.is_none(),
        });
        line.quantity = line.quantity.saturating_add(quantity);
        if stack.len() < MAX_DEPTH && !stack.contains(&target) {
            stack.push(target.clone());
            add(&target, quantity, links_of, totals, stack);
            stack.pop();
        }
    }
}

/// Every design `root` uses, directly or through others, without `root`.
pub fn used(root: &str, links_of: impl Fn(&str) -> Option<Vec<String>>) -> Vec<String> {
    lines(root, links_of).into_iter().map(|l| l.path).filter(|p| p != root).collect()
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
    use std::collections::HashMap;

    fn links(map: &[(&str, &[&str])]) -> impl Fn(&str) -> Option<Vec<String>> {
        let map: HashMap<String, Vec<String>> = map.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect())).collect();
        move |p: &str| map.get(p).cloned()
    }

    #[test]
    fn counts_parts_through_sub_assemblies() {
        // A box: a lid, two hinges (each with two screws), four screws of its own.
        let of = links(&[
            ("box", &["lid", "hinge", "hinge", "screw", "screw", "screw", "screw"]),
            ("hinge", &["screw", "screw", "pin"]),
            ("lid", &[]),
            ("screw", &[]),
            ("pin", &[]),
        ]);
        let lines = lines("box", of);
        let got: Vec<(&str, u32, bool)> = lines.iter().map(|l| (l.path.as_str(), l.quantity, l.assembly)).collect();
        assert_eq!(got, [("hinge", 2, true), ("lid", 1, false), ("pin", 2, false), ("screw", 8, false)]);
    }

    #[test]
    fn missing_designs_and_loops_are_shown_not_followed() {
        let of = links(&[("a", &["b", "gone"]), ("b", &["a"])]);
        let lines = lines("a", of);
        let got: Vec<(&str, u32, bool)> = lines.iter().map(|l| (l.path.as_str(), l.quantity, l.missing)).collect();
        assert_eq!(got, [("a", 1, false), ("b", 1, false), ("gone", 1, true)]);
        assert_eq!(used("a", links(&[("a", &["b", "gone"]), ("b", &["a"])])), ["b", "gone"]);
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
