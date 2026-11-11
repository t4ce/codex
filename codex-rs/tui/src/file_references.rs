//! Bounded path-only references shared by UI4 drops and the composer.
//! Labels quote paths reversibly, including controls; neither decoding nor staging reads files.

pub(crate) const LIMIT: usize = 16;
const MAX_PAYLOAD_BYTES: usize = 3072;
const MAX_LABEL_BYTES: usize = 12 * 1024;
const PREFIX: &str = "[File/folder reference: ";

pub(crate) fn decode(mut bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return None;
    }
    let mut paths = Vec::new();
    while !bytes.is_empty() {
        let len = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let end = 4usize.checked_add(len)?;
        let path = std::str::from_utf8(bytes.get(4..end)?).ok()?;
        if path.is_empty() || path.contains('\0') || paths.len() == LIMIT {
            return None;
        }
        paths.push(path.to_owned());
        bytes = &bytes[end..];
    }
    (!paths.is_empty()).then_some(paths)
}

/// Prepare the whole group before editing. Live elements are the source of truth,
/// so deletion, undo, recall and rich copy/paste need no second attachment registry.
pub(crate) fn plan(elements: &[String], paths: &[String]) -> Option<Vec<String>> {
    if paths.is_empty() || paths.len() > LIMIT {
        return None;
    }
    let mut existing: Vec<_> = elements
        .iter()
        .filter(|label| label.starts_with(PREFIX) && label.ends_with(']'))
        .cloned()
        .collect();
    let mut incoming = Vec::new();
    for path in paths {
        if path.is_empty() || path.contains('\0') {
            return None;
        }
        let label = format!("{PREFIX}{path:?}]");
        if !existing.contains(&label) {
            existing.push(label.clone());
            incoming.push(label);
        }
        if existing.len() > LIMIT
            || existing.iter().map(String::len).sum::<usize>() > MAX_LABEL_BYTES
        {
            return None;
        }
    }
    Some(incoming)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(paths: &[&str]) -> Vec<u8> {
        paths
            .iter()
            .flat_map(|path| {
                (path.len() as u32)
                    .to_le_bytes()
                    .into_iter()
                    .chain(path.bytes())
            })
            .collect()
    }

    #[test]
    fn termdir_wire_preserves_paths_and_rejects_incomplete_groups() {
        let path = "4:/a folder/§file\n\".rs";
        assert_eq!(decode(&encode(&[path])).unwrap(), [path]);
        for bytes in [
            vec![],
            vec![1],
            encode(&[""]),
            encode(&["\0"]),
            vec![1, 0, 0, 0, 255],
            encode(&["okay", "partial"]).into_iter().take(15).collect(),
            encode(&[&"x".repeat(MAX_PAYLOAD_BYTES)]),
        ] {
            assert!(decode(&bytes).is_none(), "{bytes:?}");
        }
        assert!(decode(&encode(&["/a"; LIMIT + 1])).is_none());
    }

    #[test]
    fn labels_are_literal_deduplicated_and_group_limits_are_atomic() {
        let paths = vec!["4:/foo\n\u{1b}[31m\".rs".to_owned()];
        let labels = plan(&[], &paths).unwrap();
        assert_eq!(
            labels,
            ["[File/folder reference: \"4:/foo\\n\\u{1b}[31m\\\".rs\"]"]
        );
        assert!(plan(&labels, &paths).unwrap().is_empty());
        assert_eq!(plan(&[], &["/a".into(), "/a".into()]).unwrap().len(), 1);
        let full = plan(
            &[],
            &(0..LIMIT).map(|n| format!("/{n}")).collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(plan(&full, &["/new".into()]).is_none());
        assert!(plan(&[], &["x".repeat(MAX_LABEL_BYTES)]).is_none());
        assert!(plan(&[], &["/a".into(), "\0".into()]).is_none());
        // Removing an element allows it to be attached again.
        assert_eq!(plan(&[], &paths).unwrap(), labels);
    }
}
