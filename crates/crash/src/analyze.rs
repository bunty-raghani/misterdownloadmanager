//! A minidump made readable: every thread's stack with function, file and line where symbols are
//! found, what went wrong, and the system it happened on.
//!
//! Symbols come from two places, tried in turn: Breakpad `.sym` files in symbol folders (the
//! release's symbols, made in CI and fetched on demand: `symbols.rs`), and the native debug info
//! beside the program (a build from source: its PDB, DWARF or dSYM).

use std::path::{Path, PathBuf};

use minidump::Module;
use minidump_unwind::{MultiSymbolProvider, Symbolizer, simple_symbol_supplier};
use serde::{Deserialize, Serialize};

/// One frame of a stack.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    /// The function, when symbols are found (inlined callers are frames of their own).
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// The module's file name ("misterdownloadmanager.exe", "ntdll.dll").
    pub module: Option<String>,
    /// The address within the module (what maps it back to the symbols later).
    pub offset: u64,
}

/// One thread.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Thread {
    pub id: u32,
    pub name: Option<String>,
    pub frames: Vec<Frame>,
}

/// A loaded module, with what identifies its symbols.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModuleInfo {
    pub name: String,
    pub version: Option<String>,
    pub debug_file: Option<String>,
    pub debug_id: Option<String>,
    pub base: u64,
    pub size: u64,
}

/// What a dump says.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Analysis {
    /// The exception or signal ("EXCEPTION_ACCESS_VIOLATION_READ", "SIGSEGV / SEGV_MAPERR").
    pub reason: Option<String>,
    /// The dump was asked for (a panic, a freeze: crash-handler's simulated exception), not caused
    /// by a crash.
    pub requested: bool,
    pub address: Option<u64>,
    /// The thread the dump was taken for (the one that crashed, panicked or asked for it).
    pub requesting: Option<u32>,
    pub threads: Vec<Thread>,
    pub modules: Vec<ModuleInfo>,
    pub os: String,
    pub os_version: Option<String>,
    pub cpu: String,
    pub cpu_info: Option<String>,
    pub cpu_count: usize,
    /// Modules whose symbols weren't found (their frames show as module + offset).
    pub missing_symbols: Vec<String>,
}

impl Analysis {
    pub fn thread(&self, id: u32) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }
}

/// Read `dump` with the `.sym` folders in `symbols` and, failing them, each module's own debug
/// info.
pub fn analyze(dump: &Path, symbols: &[PathBuf]) -> Result<Analysis, String> {
    let bytes = std::fs::read(dump).map_err(|e| e.to_string())?;
    // A Linux program listed as two modules is one again first (#661, modules.rs).
    let dump = minidump::Minidump::read(crate::modules::mend(bytes)).map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let system_info = dump
            .get_stream::<minidump::MinidumpSystemInfo>()
            .map_err(|e| e.to_string())?;
        let modules = dump
            .get_stream::<minidump::MinidumpModuleList>()
            .map_err(|e| e.to_string())?;
        let mut provider = MultiSymbolProvider::new();
        let dirs: Vec<PathBuf> = symbols.iter().filter(|d| d.is_dir()).cloned().collect();
        if !dirs.is_empty() {
            provider.add(Box::new(Symbolizer::new(simple_symbol_supplier(dirs))));
        }
        provider.add(Box::new(
            minidump_unwind::debuginfo::DebugInfoSymbolProvider::new(&system_info, &modules).await,
        ));
        let state = minidump_processor::process_minidump(&dump, &provider)
            .await
            .map_err(|e| e.to_string())?;
        Ok(from_state(&state))
    })
}

fn from_state(state: &minidump_processor::ProcessState) -> Analysis {
    let mut missing = std::collections::BTreeSet::new();
    let threads = state
        .threads
        .iter()
        .map(|t| Thread {
            id: t.thread_id,
            name: t.thread_name.clone().filter(|n| !n.is_empty()),
            frames: t
                .frames
                .iter()
                .flat_map(|f| {
                    let module = f.module.as_ref().map(|m| file_name(&m.code_file()));
                    let offset = f
                        .module
                        .as_ref()
                        .map_or(f.instruction, |m| f.instruction - m.base_address());
                    if f.function_name.is_none()
                        && let Some(m) = &module
                    {
                        missing.insert(m.clone());
                    }
                    // Inlined calls first (innermost), then the frame's own function.
                    let mut out: Vec<Frame> = f
                        .inlines
                        .iter()
                        .map(|i| Frame {
                            function: Some(i.function_name.clone()),
                            file: i.source_file_name.clone(),
                            line: i.source_line,
                            module: module.clone(),
                            offset,
                        })
                        .collect();
                    out.push(Frame {
                        function: f.function_name.clone(),
                        file: f.source_file_name.clone(),
                        line: f.source_line,
                        module: module.clone(),
                        offset,
                    });
                    out
                })
                .collect(),
        })
        .collect();
    let modules = state
        .modules
        .iter()
        .map(|m| ModuleInfo {
            name: file_name(&m.code_file()),
            version: m.version().map(|v| v.to_string()),
            debug_file: m.debug_file().map(|d| file_name(&d)),
            debug_id: m.debug_identifier().map(|d| d.breakpad().to_string()),
            base: m.base_address(),
            size: m.size(),
        })
        .collect();
    let si = &state.system_info;
    let reason = state.exception_info.as_ref().map(|e| e.reason.to_string());
    Analysis {
        requested: reason.as_deref().is_some_and(is_requested),
        reason,
        address: state.exception_info.as_ref().map(|e| e.address.0),
        requesting: state
            .requesting_thread
            .and_then(|i| state.threads.get(i))
            .map(|t| t.thread_id),
        threads,
        modules,
        os: si.os.to_string(),
        os_version: si.os_version.clone(),
        cpu: si.cpu.to_string(),
        cpu_info: si.cpu_info.clone(),
        cpu_count: si.cpu_count,
        missing_symbols: missing.into_iter().collect(),
    }
}

/// The reason of a dump asked for: crash-handler's simulated exception (Windows' user code
/// 0x0cca11ed, macOS' simulated exception) or the signal MDM simulates on Linux (SIGUSR2).
fn is_requested(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("0x0cca11ed")
        || r.contains("0xcca11ed")
        || r.contains("sigusr2")
        || r.contains("simulated")
}

/// The last part of a path, whichever its separators.
fn file_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_names_from_either_kind_of_path() {
        assert_eq!(super::file_name(r"C:\Program Files\MDM\mdm.exe"), "mdm.exe");
        assert_eq!(super::file_name("/usr/lib/libc.so.6"), "libc.so.6");
        assert_eq!(super::file_name("plain"), "plain");
    }
}
