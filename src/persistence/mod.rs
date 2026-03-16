//! Persistence helpers for serialising and deserialising CLTS instances.
//!
//! Snapshots use a compact binary format with a shared string table so large
//! automata can be spilled to disk before their in-memory footprint risks OOM.
//! The format is versioned and optimised for size/IO rather than human
//! readability.

use crate::clts::{Clts, CltsError, DefaultLabelIdx, DefaultStateIdx, LabelControllability};
use crate::context::Context;
use std::collections::{HashMap, hash_map::Entry};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use thiserror::Error;

mod io;
mod serialize;

use self::io::{BinaryRead, BinaryWrite};
use self::serialize::{Deserializable, Serializable};

const MAGIC: &[u8; 8] = b"CLTSBIN\0";
const VERSION: u32 = 2;
const SEG_MAGIC: &[u8; 8] = b"CLTSSEG\0";
const SEG_VERSION: u32 = 1;
const CTX_MAGIC: &[u8; 8] = b"CTXBIN\0\0";
const CTX_VERSION: u32 = 1;

/// Persistence related failures.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to rebuild CLTS: {0}")]
    Clts(#[from] CltsError),
    #[error("invalid snapshot: {0}")]
    InvalidSnapshot(String),
    #[error("missing segment index for {0}")]
    MissingSegmentIndex(String),
    #[error("invalid segment index: {0}")]
    InvalidSegmentIndex(String),
}

/// Stored representation of label controllability inside snapshots.
///
/// This is kept separate from [`LabelControllability`] so the on-disk encoding
/// stays stable even if the public enum grows additional variants in the
/// future.
#[derive(Debug, Clone, Copy)]
enum StoredLabelClass {
    Controllable,
    Internal,
    Uncontrollable,
}

impl StoredLabelClass {
    fn to_controllability(self) -> LabelControllability {
        match self {
            StoredLabelClass::Controllable => LabelControllability::Controllable,
            StoredLabelClass::Internal => LabelControllability::Internal,
            StoredLabelClass::Uncontrollable => LabelControllability::Uncontrollable,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            StoredLabelClass::Controllable => 0,
            StoredLabelClass::Internal => 1,
            StoredLabelClass::Uncontrollable => 2,
        }
    }

    fn from_u8(value: u8) -> Result<Self, PersistenceError> {
        match value {
            0 => Ok(StoredLabelClass::Controllable),
            1 => Ok(StoredLabelClass::Internal),
            2 => Ok(StoredLabelClass::Uncontrollable),
            other => Err(PersistenceError::InvalidSnapshot(format!(
                "invalid label controllability tag {other}"
            ))),
        }
    }
}

/// Serialises `clts` to `path` in a compact binary format.
///
/// # Parameters
///
/// * `clts`: The CLTS instance to save.
/// * `path`: The path to save the CLTS instance to.
///
/// # Returns
///
/// `Ok(())` when the CLTS instance is saved successfully.
///
/// # Errors
///
/// Returns an error if the CLTS instance cannot be saved.
pub fn save_clts_to_path<P: AsRef<Path>>(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    path: P,
) -> Result<(), PersistenceError> {
    let snapshot = BinarySnapshot::from_clts(clts);
    let segment_index = snapshot.segment_index();
    let path_ref = path.as_ref();
    let mut writer = BufWriter::new(fs::File::create(path_ref)?);
    snapshot.write_to(&mut writer)?;
    writer.flush()?;
    drop(writer);
    if segment_index.is_empty() {
        remove_segment_index(path_ref)?;
    } else {
        write_segment_index(path_ref, &segment_index)?;
    }
    Ok(())
}

/// Deserialises a CLTS snapshot stored at `path`.
///
/// # Parameters
///
/// * `path`: The path to load the CLTS instance from.
///
/// # Returns
///
/// The CLTS instance.
///
/// # Errors
///
/// Returns an error if the CLTS instance cannot be loaded.
pub fn load_clts_from_path<P: AsRef<Path>>(
    path: P,
) -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, PersistenceError> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    let snapshot = BinarySnapshot::read_from(&mut reader)?;
    snapshot.into_clts()
}

/// Spills the CLTS to `path` when the serialised payload exceeds `limit_bytes`.
/// Returns the number of bytes written when spilling occurs.
pub fn maybe_spill_clts<P: AsRef<Path>>(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    limit_bytes: usize,
    path: P,
) -> Result<Option<usize>, PersistenceError> {
    let snapshot = BinarySnapshot::from_clts(clts);
    let segment_index = snapshot.segment_index();
    let mut buffer = Vec::new();
    snapshot.write_to(&mut buffer)?;
    let path_ref = path.as_ref();
    if buffer.len() >= limit_bytes {
        let mut writer = BufWriter::new(fs::File::create(path_ref)?);
        writer.write_all(&buffer)?;
        writer.flush()?;
        drop(writer);
        if segment_index.is_empty() {
            remove_segment_index(path_ref)?;
        } else {
            write_segment_index(path_ref, &segment_index)?;
        }
        Ok(Some(buffer.len()))
    } else {
        remove_segment_index(path_ref)?;
        Ok(None)
    }
}

/// Serialises the full [`Context`] (all registered CLTS instances) to `path`.
///
/// The snapshot format is:
/// - 8-byte magic header: `CTXBIN\0\0`
/// - `u32` version
/// - `u32` CLTS count
/// - For each CLTS:
///   - `u32` length of the UTF-8 name in bytes
///   - raw name bytes
///   - `u64` length of the embedded `CLTSBIN` snapshot
///   - raw `CLTSBIN` payload
///
/// Embedded CLTS snapshots use the same format as [`save_clts_to_path`].
pub fn save_context_to_path<P: AsRef<Path>>(
    context: &Context,
    path: P,
) -> Result<(), PersistenceError> {
    let path_ref = path.as_ref();
    let mut writer = BufWriter::new(fs::File::create(path_ref)?);

    writer.write_all(CTX_MAGIC)?;
    writer.write_u32(CTX_VERSION)?;

    let mut names = context.clts_names();
    names.sort();
    writer.write_u32(names.len() as u32)?;

    for name in names {
        let name_bytes = name.as_bytes();
        writer.write_u32(name_bytes.len() as u32)?;
        writer.write_all(name_bytes)?;

        let clts = context.clts(&name).ok_or_else(|| {
            PersistenceError::InvalidSnapshot(format!(
                "CLTS '{name}' missing from context during save"
            ))
        })?;

        let snapshot = BinarySnapshot::from_clts(clts);
        let mut buffer = Vec::new();
        snapshot.write_to(&mut buffer)?;

        let len = buffer.len() as u64;
        writer.write_all(&len.to_le_bytes())?;
        writer.write_all(&buffer)?;
    }

    writer.flush()?;
    Ok(())
}

/// Deserialises a [`Context`] snapshot stored at `path`.
///
/// This reconstructs each embedded CLTS from its `CLTSBIN` snapshot and then
/// feeds them through [`Context::builder`] / `ContextBuilder::finish_with_checks`
/// so shared label stores, controllable alphabets, and global variables are
/// recomputed.
pub fn load_context_from_path<P: AsRef<Path>>(path: P) -> Result<Context, PersistenceError> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != CTX_MAGIC {
        return Err(PersistenceError::InvalidSnapshot(
            "context snapshot header mismatch".into(),
        ));
    }

    let version = reader.read_u32()?;
    if version != CTX_VERSION {
        return Err(PersistenceError::InvalidSnapshot(format!(
            "unsupported context snapshot version {version}"
        )));
    }

    let entry_count = reader.read_u32()? as usize;
    let mut builder = Context::builder();

    for _ in 0..entry_count {
        let name_len = reader.read_u32()? as usize;
        let mut name_buf = vec![0u8; name_len];
        reader.read_exact(&mut name_buf)?;
        let name = String::from_utf8(name_buf).map_err(|_| {
            PersistenceError::InvalidSnapshot("context name is not valid UTF-8".into())
        })?;

        let mut len_buf = [0u8; 8];
        reader.read_exact(&mut len_buf)?;
        let snapshot_len = u64::from_le_bytes(len_buf) as usize;
        let mut snapshot_buf = vec![0u8; snapshot_len];
        reader.read_exact(&mut snapshot_buf)?;

        let mut cursor = std::io::Cursor::new(snapshot_buf);
        let snapshot = BinarySnapshot::read_from(&mut cursor)?;
        let clts = snapshot.into_clts()?;

        builder = builder.register_clts(name, clts);
    }

    builder.finish_with_checks().map_err(|err| {
        PersistenceError::InvalidSnapshot(format!("failed to rebuild context: {err}"))
    })
}

/// Binary snapshot of a CLTS instance.
///
/// This struct contains the string table, states, transitions, and label
/// controllability metadata for a CLTS instance. The core layout (header,
/// string table, states, transitions) is kept stable so older readers can
/// still locate transition segments; label metadata is appended after the
/// transitions for newer versions.
///
/// # Fields
///
/// * `string_table`: The string table.
/// * `states`: The states.
/// * `transitions`: The transitions.
/// * `label_classes`: Label controllability entries keyed by symbol sets.
#[derive(Debug)]
struct BinarySnapshot {
    string_table: Vec<String>,
    states: Vec<StateEntry>,
    transitions: Vec<TransitionEntry>,
    label_classes: Vec<LabelClassEntry>,
}

/// Stored label controllability entry.
///
/// Each entry describes a single label by the indices of its symbol payload
/// inside `string_table` plus its controllability classification.
#[derive(Debug)]
struct LabelClassEntry {
    class: StoredLabelClass,
    symbols: Vec<u32>,
}

impl BinarySnapshot {
    fn from_clts(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>) -> Self {
        let mut string_intern = StringIntern::default();
        let mut states = Vec::with_capacity(clts.state_count());
        let mut state_indices = HashMap::new();

        for (idx, state_id) in clts.states().enumerate() {
            let name = clts
                .state_name(state_id)
                .expect("state must have a name")
                .to_owned();
            let name_idx = string_intern.intern(name);
            let variables = clts
                .state_variables(state_id)
                .into_iter()
                .map(|var| string_intern.intern(var))
                .collect();
            let entry = StateEntry {
                name_idx,
                initial: clts.initial_states().contains(&state_id),
                variables,
            };
            state_indices.insert(state_id.index(), idx as u32);
            states.push(entry);
        }

        let mut transitions = Vec::new();
        for state_id in clts.states() {
            let from_idx = *state_indices
                .get(&state_id.index())
                .expect("state index recorded");
            for transition in clts.outgoing(state_id) {
                let to_idx = *state_indices
                    .get(&transition.target().index())
                    .expect("target index recorded");
                let label_sets: Vec<Vec<u32>> = transition
                    .labels()
                    .iter()
                    .map(|label_id| {
                        clts.label_payload(*label_id)
                            .map(|payload| {
                                payload
                                    .iter()
                                    .map(|s| string_intern.intern(s.clone()))
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect();
                transitions.push(TransitionEntry {
                    from_idx,
                    to_idx,
                    labels: label_sets,
                });
            }
        }

        // Capture label controllability metadata so the snapshot can round-trip
        // controllable/uncontrollable/internal classification. This is derived
        // from the CLTS itself rather than re-inferring it at load time.
        let mut label_classes = Vec::new();

        for &label_id in clts.controllable_alphabet() {
            if let Some(payload) = clts.label_payload(label_id) {
                let symbols = payload
                    .iter()
                    .map(|s| string_intern.intern(s.clone()))
                    .collect();
                label_classes.push(LabelClassEntry {
                    class: StoredLabelClass::Controllable,
                    symbols,
                });
            }
        }

        for &label_id in clts.internal_alphabet() {
            if let Some(payload) = clts.label_payload(label_id) {
                let symbols = payload
                    .iter()
                    .map(|s| string_intern.intern(s.clone()))
                    .collect();
                label_classes.push(LabelClassEntry {
                    class: StoredLabelClass::Internal,
                    symbols,
                });
            }
        }

        for &label_id in clts.uncontrollable_alphabet() {
            if let Some(payload) = clts.label_payload(label_id) {
                let symbols = payload
                    .iter()
                    .map(|s| string_intern.intern(s.clone()))
                    .collect();
                label_classes.push(LabelClassEntry {
                    class: StoredLabelClass::Uncontrollable,
                    symbols,
                });
            }
        }

        Self {
            string_table: string_intern.into_vec(),
            states,
            transitions,
            label_classes,
        }
    }

    fn prefix_size(&self) -> u64 {
        let mut size = MAGIC.len() as u64; // magic
        size += 4; // version
        size += 4; // string count
        for entry in &self.string_table {
            size += 4; // string length
            size += entry.len() as u64;
        }
        size += 4; // state count
        for state in &self.states {
            size += 4; // name index
            size += 1; // initial flag (u8)
            size += 4; // variable count
            size += (state.variables.len() as u64) * 4; // variable indices
        }
        size += 4; // transition count
        size
    }

    fn transition_size(entry: &TransitionEntry) -> u64 {
        let mut size = 4 + 4 + 4; // from, to, label count
        for label in &entry.labels {
            size += 4; // symbol count
            size += (label.len() as u64) * 4;
        }
        size
    }

    fn segment_index(&self) -> TransitionSegmentIndex {
        let mut segments = Vec::new();
        let mut offset = self.prefix_size();
        let mut current_from = None;
        let mut current_offset = offset;
        let mut current_len = 0u64;
        let mut current_count = 0u32;

        for entry in &self.transitions {
            if current_from != Some(entry.from_idx) {
                if let Some(from) = current_from {
                    segments.push(TransitionSegmentInfo {
                        from_state: from,
                        offset: current_offset,
                        len_bytes: current_len,
                        transitions: current_count,
                    });
                }
                current_from = Some(entry.from_idx);
                current_offset = offset;
                current_len = 0;
                current_count = 0;
            }

            let entry_size = Self::transition_size(entry);
            offset += entry_size;
            current_len += entry_size;
            current_count += 1;
        }

        if let Some(from) = current_from {
            segments.push(TransitionSegmentInfo {
                from_state: from,
                offset: current_offset,
                len_bytes: current_len,
                transitions: current_count,
            });
        }

        TransitionSegmentIndex { segments }
    }

    fn into_clts(self) -> Result<Clts<DefaultStateIdx, DefaultLabelIdx>, PersistenceError> {
        let Self {
            string_table,
            states,
            transitions,
            label_classes,
        } = self;

        let mut builder = Clts::builder();
        let mut state_lookup = Vec::with_capacity(states.len());

        for state in &states {
            let name = string_table.get(state.name_idx as usize).ok_or_else(|| {
                PersistenceError::InvalidSnapshot("state name index out of bounds".into())
            })?;
            builder.state(name);
        }

        for state in &states {
            let name = string_table.get(state.name_idx as usize).ok_or_else(|| {
                PersistenceError::InvalidSnapshot("state name index out of bounds".into())
            })?;
            let id = builder
                .state_id_or_insert(name)
                .expect("state added during initial pass");
            if state.initial {
                builder.initial_state_id(id);
            }
            if !state.variables.is_empty() {
                let vars: Vec<String> = state
                    .variables
                    .iter()
                    .map(|idx| {
                        string_table
                            .get(*idx as usize)
                            .ok_or_else(|| {
                                PersistenceError::InvalidSnapshot(
                                    "variable index out of bounds".into(),
                                )
                            })
                            .cloned()
                    })
                    .collect::<Result<_, _>>()?;
                builder.with_variables_for_state(id, vars);
            }
            state_lookup.push(id);
        }

        for transition in &transitions {
            let from = state_lookup
                .get(transition.from_idx as usize)
                .ok_or_else(|| {
                    PersistenceError::InvalidSnapshot(
                        "transition source index out of bounds".into(),
                    )
                })?;
            let to = state_lookup
                .get(transition.to_idx as usize)
                .ok_or_else(|| {
                    PersistenceError::InvalidSnapshot(
                        "transition target index out of bounds".into(),
                    )
                })?;

            let mut label_ids = Vec::with_capacity(transition.labels.len());
            for label in &transition.labels {
                let symbols: Vec<&str> = label
                    .iter()
                    .map(|idx| {
                        string_table
                            .get(*idx as usize)
                            .map(|s| s.as_str())
                            .ok_or_else(|| {
                                PersistenceError::InvalidSnapshot(
                                    "label symbol index out of bounds".into(),
                                )
                            })
                    })
                    .collect::<Result<_, _>>()?;
                let label_id = builder.labels().intern(symbols)?;
                label_ids.push(label_id);
            }

            builder.transition_ids(*from, &label_ids, *to);
        }
        // Rebuild label controllability information if present in the snapshot.
        // This ensures that controllable/uncontrollable/internal alphabets and
        // derived data structures (like uncontrollable_groups) are restored
        // rather than re-inferred from defaults. At this point all labels used
        // by transitions have already been interned, so we only look up their
        // identifiers.
        for class_entry in &label_classes {
            let symbols: Vec<&str> = class_entry
                .symbols
                .iter()
                .map(|idx| {
                    string_table
                        .get(*idx as usize)
                        .map(|s| s.as_str())
                        .ok_or_else(|| {
                            PersistenceError::InvalidSnapshot(
                                "label symbol index out of bounds in controllability table".into(),
                            )
                        })
                })
                .collect::<Result<_, _>>()?;
            let label_id = builder.labels().intern(symbols)?;
            let controllability = class_entry.class.to_controllability();
            builder.set_label_controllability(label_id, controllability);
        }

        builder.build().map_err(PersistenceError::from)
    }

    fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        writer.write_all(MAGIC)?;
        writer.write_u32(VERSION)?;

        writer.write_u32(self.string_table.len() as u32)?;
        for entry in &self.string_table {
            let bytes = entry.as_bytes();
            writer.write_u32(bytes.len() as u32)?;
            writer.write_all(bytes)?;
        }

        writer.write_u32(self.states.len() as u32)?;
        for state in &self.states {
            state.serialize(writer)?;
        }

        writer.write_u32(self.transitions.len() as u32)?;
        for transition in &self.transitions {
            transition.serialize(writer)?;
        }

        // Write label controllability metadata (added in VERSION 2). Older
        // readers that only understand VERSION 1 will ignore this section.
        writer.write_u32(self.label_classes.len() as u32)?;
        for entry in &self.label_classes {
            writer.write_u8(entry.class.to_u8())?;
            writer.write_u32(entry.symbols.len() as u32)?;
            for symbol in &entry.symbols {
                writer.write_u32(*symbol)?;
            }
        }

        Ok(())
    }

    fn read_from<R: Read>(reader: &mut R) -> Result<Self, PersistenceError> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(PersistenceError::InvalidSnapshot(
                "snapshot header mismatch".into(),
            ));
        }

        let version = reader.read_u32()?;
        if version > VERSION {
            return Err(PersistenceError::InvalidSnapshot(format!(
                "unsupported snapshot version {version}"
            )));
        }

        let string_count = reader.read_u32()? as usize;
        let mut string_table = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            let len = reader.read_u32()? as usize;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            let string = String::from_utf8(buf).map_err(|_| {
                PersistenceError::InvalidSnapshot("string table entry is not valid UTF-8".into())
            })?;
            string_table.push(string);
        }

        let state_count = reader.read_u32()? as usize;
        let mut states = Vec::with_capacity(state_count);
        for _ in 0..state_count {
            states.push(StateEntry::deserialize(reader)?);
        }

        let transition_count = reader.read_u32()? as usize;
        let mut transitions = Vec::with_capacity(transition_count);
        for _ in 0..transition_count {
            transitions.push(TransitionEntry::deserialize(reader)?);
        }
        let label_classes = if version >= 1 {
            let class_count = reader.read_u32()? as usize;
            let mut classes = Vec::with_capacity(class_count);
            for _ in 0..class_count {
                let tag = reader.read_u8()?;
                let class = StoredLabelClass::from_u8(tag)?;
                let symbol_count = reader.read_u32()? as usize;
                let mut symbols = Vec::with_capacity(symbol_count);
                for _ in 0..symbol_count {
                    symbols.push(reader.read_u32()?);
                }
                classes.push(LabelClassEntry { class, symbols });
            }
            classes
        } else {
            Vec::new()
        };

        Ok(Self {
            string_table,
            states,
            transitions,
            label_classes,
        })
    }
}

#[derive(Debug, Default)]
struct StringIntern {
    map: HashMap<String, u32>,
    entries: Vec<String>,
}

impl StringIntern {
    fn intern(&mut self, value: String) -> u32 {
        if let Some(&idx) = self.map.get(&value) {
            return idx;
        }
        let idx = self.entries.len() as u32;
        self.entries.push(value.clone());
        self.map.insert(value, idx);
        idx
    }

    fn into_vec(self) -> Vec<String> {
        self.entries
    }
}

#[derive(Debug)]
struct StateEntry {
    name_idx: u32,
    initial: bool,
    variables: Vec<u32>,
}

impl Serializable for StateEntry {
    fn serialize<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        writer.write_u32(self.name_idx)?;
        writer.write_bool(self.initial)?;
        writer.write_u32(self.variables.len() as u32)?;
        for var in &self.variables {
            writer.write_u32(*var)?;
        }
        Ok(())
    }
}

impl Deserializable for StateEntry {
    fn deserialize<R: Read>(reader: &mut R) -> Result<Self, PersistenceError> {
        let name_idx = reader.read_u32()?;
        let initial = reader.read_bool()?;
        let var_count = reader.read_u32()? as usize;
        let mut variables = Vec::with_capacity(var_count);
        for _ in 0..var_count {
            variables.push(reader.read_u32()?);
        }
        Ok(StateEntry {
            name_idx,
            initial,
            variables,
        })
    }
}

#[derive(Debug, Clone)]
struct TransitionSegmentInfo {
    from_state: u32,
    offset: u64,
    len_bytes: u64,
    transitions: u32,
}

#[derive(Debug, Clone)]
struct TransitionSegmentIndex {
    segments: Vec<TransitionSegmentInfo>,
}

impl TransitionSegmentIndex {
    fn find(&self, from_state: u32) -> Option<&TransitionSegmentInfo> {
        self.segments
            .iter()
            .find(|segment| segment.from_state == from_state)
    }

    fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct PrefetchCache {
    segments: HashMap<u32, Vec<PrefetchedTransition>>,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
struct TransitionEntry {
    from_idx: u32,
    to_idx: u32,
    labels: Vec<Vec<u32>>,
}

impl Serializable for TransitionEntry {
    fn serialize<W: Write>(&self, writer: &mut W) -> Result<(), PersistenceError> {
        writer.write_u32(self.from_idx)?;
        writer.write_u32(self.to_idx)?;
        writer.write_u32(self.labels.len() as u32)?;
        for label in &self.labels {
            writer.write_u32(label.len() as u32)?;
            for symbol in label {
                writer.write_u32(*symbol)?;
            }
        }
        Ok(())
    }
}

impl Deserializable for TransitionEntry {
    fn deserialize<R: Read>(reader: &mut R) -> Result<Self, PersistenceError> {
        let from_idx = reader.read_u32()?;
        let to_idx = reader.read_u32()?;
        let label_count = reader.read_u32()? as usize;
        let mut labels = Vec::with_capacity(label_count);
        for _ in 0..label_count {
            let symbol_count = reader.read_u32()? as usize;
            let mut symbols = Vec::with_capacity(symbol_count);
            for _ in 0..symbol_count {
                symbols.push(reader.read_u32()?);
            }
            labels.push(symbols);
        }
        Ok(TransitionEntry {
            from_idx,
            to_idx,
            labels,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PrefetchedTransition {
    pub from_state: u32,
    pub to_state: u32,
    pub labels: Vec<Vec<String>>,
}

pub fn prefetch_transition_segment<P: AsRef<Path>>(
    path: P,
    from_state: u32,
    cache: &mut PrefetchCache,
) -> Result<&[PrefetchedTransition], PersistenceError> {
    match cache.segments.entry(from_state) {
        Entry::Occupied(entry) => {
            let value = entry.into_mut();
            Ok(value.as_slice())
        }
        Entry::Vacant(vacant) => {
            let path = path.as_ref();
            let segment_index = read_segment_index(path)?;
            let segment = segment_index.find(from_state).ok_or_else(|| {
                PersistenceError::InvalidSegmentIndex(format!(
                    "transition segment for state {from_state} not found"
                ))
            })?;

            let file = fs::File::open(path)?;
            let mut reader = BufReader::new(file);
            let string_table = read_header_to_transitions(&mut reader)?;
            let _total_transitions = reader.read_u32()?;
            let mut file = reader.into_inner();
            file.seek(SeekFrom::Start(segment.offset))?;
            let mut reader = BufReader::new(file);

            let mut transitions = Vec::with_capacity(segment.transitions as usize);
            for _ in 0..segment.transitions {
                let from_idx = reader.read_u32()?;
                let to_idx = reader.read_u32()?;
                let label_count = reader.read_u32()? as usize;
                let mut labels = Vec::with_capacity(label_count);
                for _ in 0..label_count {
                    let symbol_count = reader.read_u32()? as usize;
                    let mut symbols = Vec::with_capacity(symbol_count);
                    for _ in 0..symbol_count {
                        let idx = reader.read_u32()? as usize;
                        let symbol = string_table
                            .get(idx)
                            .ok_or_else(|| {
                                PersistenceError::InvalidSnapshot(
                                    "label symbol index out of bounds".into(),
                                )
                            })?
                            .clone();
                        symbols.push(symbol);
                    }
                    labels.push(symbols);
                }
                transitions.push(PrefetchedTransition {
                    from_state: from_idx,
                    to_state: to_idx,
                    labels,
                });
            }

            let value = vacant.insert(transitions);
            Ok(value.as_slice())
        }
    }
}

pub fn evict_transition_segment(cache: &mut PrefetchCache, from_state: u32) -> bool {
    cache.segments.remove(&from_state).is_some()
}

fn segment_index_path(path: &Path) -> std::path::PathBuf {
    let mut seg_path = path.to_path_buf();
    match path.extension() {
        Some(ext) => {
            let mut new_ext = OsString::from(ext);
            new_ext.push(".seg");
            seg_path.set_extension(new_ext);
        }
        None => {
            seg_path.set_extension("seg");
        }
    }
    seg_path
}

fn write_segment_index(
    path: &Path,
    index: &TransitionSegmentIndex,
) -> Result<(), PersistenceError> {
    let seg_path = segment_index_path(path);
    let mut writer = BufWriter::new(fs::File::create(seg_path)?);
    writer.write_all(SEG_MAGIC)?;
    writer.write_u32(SEG_VERSION)?;
    writer.write_u32(index.segments.len() as u32)?;
    for segment in &index.segments {
        writer.write_u32(segment.from_state)?;
        writer.write_all(&segment.offset.to_le_bytes())?;
        writer.write_all(&segment.len_bytes.to_le_bytes())?;
        writer.write_u32(segment.transitions)?;
    }
    writer.flush()?;
    Ok(())
}

fn remove_segment_index(path: &Path) -> Result<(), PersistenceError> {
    let seg_path = segment_index_path(path);
    match fs::remove_file(seg_path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(PersistenceError::Io(err)),
    }
}

fn read_segment_index(path: &Path) -> Result<TransitionSegmentIndex, PersistenceError> {
    let seg_path = segment_index_path(path);
    let file = fs::File::open(&seg_path).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            PersistenceError::MissingSegmentIndex(seg_path.display().to_string())
        } else {
            PersistenceError::Io(err)
        }
    })?;
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != SEG_MAGIC {
        return Err(PersistenceError::InvalidSegmentIndex(
            "segment index header mismatch".into(),
        ));
    }

    let version = reader.read_u32()?;
    if version != SEG_VERSION {
        return Err(PersistenceError::InvalidSegmentIndex(format!(
            "unsupported segment index version {version}"
        )));
    }

    let segment_count = reader.read_u32()? as usize;
    let mut segments = Vec::with_capacity(segment_count);
    for _ in 0..segment_count {
        let from_state = reader.read_u32()?;
        let mut offset_bytes = [0u8; 8];
        reader.read_exact(&mut offset_bytes)?;
        let offset = u64::from_le_bytes(offset_bytes);
        let mut len_bytes_buf = [0u8; 8];
        reader.read_exact(&mut len_bytes_buf)?;
        let len_bytes = u64::from_le_bytes(len_bytes_buf);
        let transitions = reader.read_u32()?;
        segments.push(TransitionSegmentInfo {
            from_state,
            offset,
            len_bytes,
            transitions,
        });
    }

    Ok(TransitionSegmentIndex { segments })
}

fn read_header_to_transitions(
    reader: &mut BufReader<fs::File>,
) -> Result<Vec<String>, PersistenceError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(PersistenceError::InvalidSnapshot(
            "snapshot header mismatch".into(),
        ));
    }

    let version = reader.read_u32()?;
    if version != VERSION {
        return Err(PersistenceError::InvalidSnapshot(format!(
            "unsupported snapshot version {version}"
        )));
    }

    let string_count = reader.read_u32()? as usize;
    let mut string_table = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        let len = reader.read_u32()? as usize;
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        let string = String::from_utf8(buf).map_err(|_| {
            PersistenceError::InvalidSnapshot("string table entry is not valid UTF-8".into())
        })?;
        string_table.push(string);
    }

    let state_count = reader.read_u32()? as usize;
    for _ in 0..state_count {
        reader.read_u32()?; // name index
        reader.read_bool()?; // initial flag
        let var_count = reader.read_u32()? as usize;
        for _ in 0..var_count {
            reader.read_u32()?; // variable index
        }
    }

    Ok(string_table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::tempdir;

    fn sample_clts() -> Clts<DefaultStateIdx, DefaultLabelIdx> {
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");
        builder.with_variables("s0", ["flag"]);
        let sync = builder.labels().intern(["sync"]).unwrap();
        builder.transition("s0", &[sync], "s1");
        builder.transition("s1", &[sync], "s0");
        builder.build().unwrap()
    }

    #[test]
    fn round_trip_persistence() -> Result<(), PersistenceError> {
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("clts.bin");
        save_clts_to_path(&clts, &path)?;
        let loaded = load_clts_from_path(&path)?;
        assert!(clts.structural_eq(&loaded));
        Ok(())
    }

    #[test]
    fn round_trip_preserves_label_controllability() -> Result<(), PersistenceError> {
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");

        let uncontrollable = builder.labels().intern(["u"]).unwrap();
        let controllable = builder.labels().intern(["c"]).unwrap();
        let internal = builder.labels().intern(["i"]).unwrap();

        builder.set_label_controllability(
            uncontrollable,
            crate::clts::LabelControllability::Uncontrollable,
        );
        builder.set_label_controllability(
            controllable,
            crate::clts::LabelControllability::Controllable,
        );
        builder.set_label_controllability(internal, crate::clts::LabelControllability::Internal);

        builder.transition("s0", &[uncontrollable, controllable, internal], "s1");
        let clts = builder.build()?;

        let dir = tempdir().unwrap();
        let path = dir.path().join("clts_labels.bin");
        save_clts_to_path(&clts, &path)?;
        let loaded = load_clts_from_path(&path)?;

        // Helper to collect label payload sets for a given alphabet classification.
        fn payload_sets(
            clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
            labels: &std::collections::HashSet<crate::clts::LabelId<DefaultLabelIdx>>,
        ) -> std::collections::HashSet<Vec<String>> {
            let mut result = std::collections::HashSet::new();
            for &id in labels {
                if let Some(payload) = clts.label_payload(id) {
                    let mut symbols: Vec<String> = payload.to_vec();
                    symbols.sort();
                    result.insert(symbols);
                }
            }
            result
        }

        let orig_ctrl = payload_sets(&clts, clts.controllable_alphabet());
        let orig_unctrl = payload_sets(&clts, clts.uncontrollable_alphabet());
        let orig_internal = payload_sets(&clts, clts.internal_alphabet());

        let loaded_ctrl = payload_sets(&loaded, loaded.controllable_alphabet());
        let loaded_unctrl = payload_sets(&loaded, loaded.uncontrollable_alphabet());
        let loaded_internal = payload_sets(&loaded, loaded.internal_alphabet());

        assert_eq!(orig_ctrl, loaded_ctrl);
        assert_eq!(orig_unctrl, loaded_unctrl);
        assert_eq!(orig_internal, loaded_internal);

        Ok(())
    }

    #[test]
    fn spill_respects_threshold() -> Result<(), PersistenceError> {
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("spill.bin");
        // Extremely small threshold -> should spill.
        let spilled = maybe_spill_clts(&clts, 16, &path)?;
        assert!(spilled.is_some());
        assert!(path.exists());

        let path2 = dir.path().join("no_spill.bin");
        let spilled = maybe_spill_clts(&clts, usize::MAX, &path2)?;
        assert!(spilled.is_none());
        assert!(!path2.exists());
        Ok(())
    }

    #[test]
    fn invalid_magic_is_rejected() {
        let mut cursor = Cursor::new(vec![0u8; 16]);
        let err = BinarySnapshot::read_from(&mut cursor).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSnapshot(_)));
    }

    #[test]
    fn prefetch_and_evict_segments() -> Result<(), PersistenceError> {
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("prefetch.bin");
        save_clts_to_path(&clts, &path)?;

        let mut cache = PrefetchCache::new();
        let seg = prefetch_transition_segment(&path, 0, &mut cache)?;
        assert_eq!(seg.len(), 1);
        assert_eq!(seg[0].from_state, 0);
        assert_eq!(seg[0].to_state, 1);
        assert!(evict_transition_segment(&mut cache, 0));
        assert!(!evict_transition_segment(&mut cache, 0));
        Ok(())
    }

    #[test]
    fn prefetch_missing_segment_errors() -> Result<(), PersistenceError> {
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("prefetch_missing.bin");
        save_clts_to_path(&clts, &path)?;

        let mut cache = PrefetchCache::new();
        let err = prefetch_transition_segment(&path, 42, &mut cache).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSegmentIndex(_)));
        Ok(())
    }

    #[test]
    fn save_load_error_handling() {
        // Test save_clts_to_path error handling (lines 38-55)
        let clts = sample_clts();

        // Test invalid path (parent directory doesn't exist)
        let invalid_path = std::path::Path::new("/nonexistent/path/clts.bin");
        let result = save_clts_to_path(&clts, invalid_path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PersistenceError::Io(_)));

        // Test load_clts_from_path with non-existent file (lines 58-64)
        let result = load_clts_from_path("/nonexistent/file.bin");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PersistenceError::Io(_)));
    }

    #[test]
    fn maybe_spill_clts_edge_cases() -> Result<(), PersistenceError> {
        // Test maybe_spill_clts with various thresholds (lines 68-93)
        let clts = sample_clts();
        let dir = tempdir().unwrap();

        // Test with threshold exactly at size
        let path1 = dir.path().join("spill_exact.bin");
        let snapshot = BinarySnapshot::from_clts(&clts);
        let mut buffer = Vec::new();
        snapshot.write_to(&mut buffer)?;
        let exact_size = buffer.len();

        let spilled = maybe_spill_clts(&clts, exact_size, &path1)?;
        // Should spill when size >= threshold
        assert!(spilled.is_some());
        assert_eq!(spilled.unwrap(), exact_size);
        assert!(path1.exists());

        // Test with threshold one byte larger
        let path2 = dir.path().join("no_spill_exact.bin");
        let spilled = maybe_spill_clts(&clts, exact_size + 1, &path2)?;
        assert!(spilled.is_none());
        assert!(!path2.exists());

        // Test with zero threshold (should always spill)
        let path3 = dir.path().join("spill_zero.bin");
        let spilled = maybe_spill_clts(&clts, 0, &path3)?;
        assert!(spilled.is_some());
        assert!(path3.exists());

        Ok(())
    }

    #[test]
    fn invalid_snapshot_handling() {
        // Test invalid snapshot handling (lines 363-437)

        // Test invalid version
        let mut invalid_version = Vec::new();
        invalid_version.extend_from_slice(MAGIC);
        invalid_version.extend_from_slice(&(999u32.to_le_bytes())); // Invalid version
        let mut cursor = Cursor::new(invalid_version);
        let err = BinarySnapshot::read_from(&mut cursor).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSnapshot(_)));
        assert!(err.to_string().contains("unsupported snapshot version"));

        // Test invalid boolean value (lines 652-661)
        let mut invalid_bool = Vec::new();
        invalid_bool.extend_from_slice(MAGIC);
        invalid_bool.extend_from_slice(&(VERSION.to_le_bytes()));
        invalid_bool.extend_from_slice(&(0u32.to_le_bytes())); // string_count = 0
        invalid_bool.extend_from_slice(&(1u32.to_le_bytes())); // state_count = 1
        invalid_bool.extend_from_slice(&(0u32.to_le_bytes())); // name_idx = 0
        invalid_bool.push(2); // invalid boolean (should be 0 or 1)
        let mut cursor = Cursor::new(invalid_bool);
        let err = BinarySnapshot::read_from(&mut cursor).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSnapshot(_)));
        assert!(err.to_string().contains("invalid boolean"));

        // Note: Transition kind validation was removed when TransitionKind was removed from persistence
        // The format no longer includes transition kind, so this test is no longer applicable
    }

    #[test]
    fn round_trip_with_complex_clts() -> Result<(), PersistenceError> {
        // Test round-trip with more complex CLTS (multiple states, transitions, variables)
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");
        builder.state("s2");
        builder.with_variables("s0", ["var1", "var2"]);
        builder.with_variables("s1", ["var3"]);

        let alpha = builder.labels().intern(["alpha"]).unwrap();
        let beta = builder.labels().intern(["beta"]).unwrap();
        let gamma = builder.labels().intern(["gamma", "delta"]).unwrap(); // Multi-label

        builder.transition("s0", &[alpha], "s1");
        builder.transition("s1", &[beta], "s2");
        builder.transition("s2", &[gamma], "s0");
        builder.transition("s0", &[alpha, beta], "s2"); // Multiple labels

        let clts = builder.build().unwrap();

        let dir = tempdir().unwrap();
        let path = dir.path().join("complex_clts.bin");
        save_clts_to_path(&clts, &path)?;
        let loaded = load_clts_from_path(&path)?;

        assert!(clts.structural_eq(&loaded));

        // Verify specific properties
        assert_eq!(loaded.state_count(), 3);
        assert_eq!(clts.initial_states().len(), loaded.initial_states().len());

        Ok(())
    }

    #[test]
    fn segment_index_operations() -> Result<(), PersistenceError> {
        // Test segment index write/read operations (lines 679-753)
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("segment_test.bin");
        save_clts_to_path(&clts, &path)?;

        // Verify segment index file exists
        let seg_path = segment_index_path(&path);
        assert!(seg_path.exists());

        // Read segment index
        let index = read_segment_index(&path)?;
        assert!(!index.is_empty());

        // Verify we can find segments
        let segment = index.find(0);
        assert!(segment.is_some());
        let seg = segment.unwrap();
        assert_eq!(seg.from_state, 0);
        assert!(seg.transitions > 0);

        Ok(())
    }

    #[test]
    fn segment_index_error_handling() {
        // Test segment index error handling
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.bin");

        // Test missing segment index (lines 707-715)
        let err = read_segment_index(&path).unwrap_err();
        assert!(matches!(err, PersistenceError::MissingSegmentIndex(_)));

        // Test invalid segment index magic
        let seg_path = segment_index_path(&path);
        std::fs::write(&seg_path, b"INVALID\0").unwrap();
        let err = read_segment_index(&path).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSegmentIndex(_)));

        // Test invalid segment index version
        let mut invalid_seg = Vec::new();
        invalid_seg.extend_from_slice(SEG_MAGIC);
        invalid_seg.extend_from_slice(&(999u32.to_le_bytes())); // Invalid version
        std::fs::write(&seg_path, invalid_seg).unwrap();
        let err = read_segment_index(&path).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSegmentIndex(_)));
    }

    #[test]
    fn remove_segment_index_handling() -> Result<(), PersistenceError> {
        // Test remove_segment_index (lines 698-705)
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("remove_test.bin");
        save_clts_to_path(&clts, &path)?;

        let seg_path = segment_index_path(&path);
        assert!(seg_path.exists());

        // Remove segment index
        remove_segment_index(&path)?;
        assert!(!seg_path.exists());

        // Removing non-existent segment index should not error
        remove_segment_index(&path)?;

        Ok(())
    }

    #[test]
    fn string_intern_behavior() {
        // Test StringIntern intern behavior (lines 440-460)
        let mut intern = StringIntern::default();

        // Test intern returns same index for duplicate strings
        let idx1 = intern.intern("test".to_string());
        let idx2 = intern.intern("test".to_string());
        assert_eq!(idx1, idx2);

        // Test intern returns different indices for different strings
        let idx3 = intern.intern("other".to_string());
        assert_ne!(idx1, idx3);

        // Test into_vec preserves order
        let vec = intern.into_vec();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0], "test");
        assert_eq!(vec[1], "other");
    }

    #[test]
    fn snapshot_with_empty_clts() -> Result<(), PersistenceError> {
        // Test snapshot with empty CLTS (no states, no transitions)
        let builder = Clts::builder();
        let clts = builder.build().unwrap();

        let dir = tempdir().unwrap();
        let path = dir.path().join("empty_clts.bin");
        save_clts_to_path(&clts, &path)?;
        let loaded = load_clts_from_path(&path)?;

        assert!(clts.structural_eq(&loaded));
        assert_eq!(loaded.state_count(), 0);

        Ok(())
    }

    #[test]
    fn round_trip_context_snapshot() -> Result<(), PersistenceError> {
        // Build a small context with two CLTS instances.
        let mut plant_builder = Clts::builder();
        plant_builder.state("p0").initial("p0");
        let tick = plant_builder.labels().intern(["tick"]).unwrap();
        plant_builder.transition("p0", &[tick], "p0");
        let plant = plant_builder.build().unwrap();

        let mut ctrl_builder = Clts::builder();
        ctrl_builder.state("c0").initial("c0");
        let cmd = ctrl_builder.labels().intern(["cmd"]).unwrap();
        ctrl_builder
            .set_label_controllability(cmd, crate::clts::LabelControllability::Controllable);
        ctrl_builder.transition("c0", &[cmd], "c0");
        let controller = ctrl_builder.build().unwrap();

        let context = Context::builder()
            .register_clts("plant", plant)
            .register_clts("controller", controller)
            .finish_with_checks()
            .map_err(|err| {
                PersistenceError::InvalidSnapshot(format!("failed to build context: {err}"))
            })?;

        let dir = tempdir().unwrap();
        let path = dir.path().join("context.bin");
        save_context_to_path(&context, &path)?;

        let loaded = load_context_from_path(&path)?;

        let mut orig_names = context.clts_names();
        let mut loaded_names = loaded.clts_names();
        orig_names.sort();
        loaded_names.sort();
        assert_eq!(orig_names, loaded_names);

        for name in orig_names {
            let orig = context.clts(&name).unwrap();
            let rebuilt = loaded.clts(&name).unwrap();
            assert!(orig.structural_eq(rebuilt));
        }

        Ok(())
    }

    #[test]
    fn snapshot_with_epsilon_transitions() -> Result<(), PersistenceError> {
        // Test snapshot with epsilon transitions
        let mut builder = Clts::builder();
        builder.state("s0").initial("s0");
        builder.state("s1");

        // Create epsilon transition (empty label set)
        let s0_id = builder.state_id_or_insert("s0").unwrap();
        let s1_id = builder.state_id_or_insert("s1").unwrap();
        let empty_labels: Vec<_> = vec![];
        builder.transition_ids(s0_id, &empty_labels, s1_id);

        let clts = builder.build().unwrap();

        let dir = tempdir().unwrap();
        let path = dir.path().join("epsilon_clts.bin");
        save_clts_to_path(&clts, &path)?;
        let loaded = load_clts_from_path(&path)?;

        assert!(clts.structural_eq(&loaded));

        Ok(())
    }

    #[test]
    fn invalid_string_table_handling() {
        // Test invalid string table handling (lines 379-389)
        let mut invalid_utf8 = Vec::new();
        invalid_utf8.extend_from_slice(MAGIC);
        invalid_utf8.extend_from_slice(&(VERSION.to_le_bytes()));
        invalid_utf8.extend_from_slice(&(1u32.to_le_bytes())); // string_count = 1
        invalid_utf8.extend_from_slice(&(3u32.to_le_bytes())); // string length = 3
        invalid_utf8.extend_from_slice(&[0xFF, 0xFE, 0xFD]); // Invalid UTF-8

        let mut cursor = Cursor::new(invalid_utf8);
        let err = BinarySnapshot::read_from(&mut cursor).unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSnapshot(_)));
        assert!(err.to_string().contains("not valid UTF-8"));
    }

    #[test]
    fn snapshot_index_bounds_checking() {
        // Test snapshot index bounds checking (lines 248-319)
        // The bounds checking happens in into_clts, not read_from
        // So we need to create a valid snapshot, then corrupt it to test bounds checking
        let clts = sample_clts();
        let dir = tempdir().unwrap();
        let path = dir.path().join("bounds_test.bin");
        save_clts_to_path(&clts, &path).unwrap();

        // Read the snapshot and corrupt a state name index
        let mut reader = BufReader::new(fs::File::open(&path).unwrap());
        let mut snapshot = BinarySnapshot::read_from(&mut reader).unwrap();

        // Corrupt a state name index to be out of bounds
        if !snapshot.states.is_empty() {
            snapshot.states[0].name_idx = 99999; // Out of bounds index
        }

        // Now into_clts should fail with bounds error
        let err = snapshot.into_clts().unwrap_err();
        assert!(matches!(err, PersistenceError::InvalidSnapshot(_)));
        assert!(err.to_string().contains("out of bounds"));
    }
}
