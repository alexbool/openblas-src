//! Check make results

use crate::error::*;
use std::{
    collections::HashSet,
    fs,
    hash::Hash,
    io::{self, BufRead},
    path::*,
    process::Command,
};

/// Parse compiler linker flags, `-L` and `-l`
///
/// - Search paths defined by `-L` will be removed if not exists,
///   and will be canonicalize
///
/// ```
/// use openblas_build::*;
/// let info = LinkFlags::parse("-L/usr/lib/gcc/x86_64-pc-linux-gnu/10.2.0 -L/usr/lib/gcc/x86_64-pc-linux-gnu/10.2.0/../../../../lib -L/lib/../lib -L/usr/lib/../lib -L/usr/lib/gcc/x86_64-pc-linux-gnu/10.2.0/../../..  -lc");
/// assert_eq!(info.libs, vec!["c"]);
/// ```
#[derive(Debug, Clone, Default)]
pub struct LinkFlags {
    /// Existing paths specified by `-L`
    pub search_paths: Vec<PathBuf>,
    /// Libraries specified by `-l`
    pub libs: Vec<String>,
}

fn as_sorted_vec<T: Hash + Ord>(set: HashSet<T>) -> Vec<T> {
    let mut v: Vec<_> = set.into_iter().collect();
    v.sort();
    v
}

impl LinkFlags {
    pub fn parse(line: &str) -> Result<Self, Error> {
        let mut search_paths = HashSet::new();
        let mut libs = HashSet::new();
        for entry in line.split(" ") {
            if entry.starts_with("-L") {
                let path = PathBuf::from(entry.trim_start_matches("-L"));
                if !path.exists() {
                    continue;
                }
                search_paths.insert(
                    path.canonicalize()
                        .map_err(|_| Error::CannotCanonicalizePath { path })?,
                );
            }
            if entry.starts_with("-l") {
                libs.insert(entry.trim_start_matches("-l").into());
            }
        }
        Ok(LinkFlags {
            search_paths: as_sorted_vec(search_paths),
            libs: as_sorted_vec(libs),
        })
    }
}

/// Parse Makefile.conf which generated by OpenBLAS make system
#[derive(Debug, Clone, Default)]
pub struct MakeConf {
    pub os_name: String,
    pub no_fortran: bool,
    pub c_extra_libs: LinkFlags,
    pub f_extra_libs: LinkFlags,
}

impl MakeConf {
    /// Parse from file
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let mut detail = MakeConf::default();
        let f = fs::File::open(&path).map_err(|_| Error::MakeConfNotExist {
            out_dir: path.as_ref().to_owned(),
        })?;
        let buf = io::BufReader::new(f);
        for line in buf.lines() {
            let line = line.expect("Makefile.conf should not include non-UTF8 string");
            if line.len() == 0 {
                continue;
            }
            let entry: Vec<_> = line.split("=").collect();
            if entry.len() != 2 {
                continue;
            }
            match entry[0] {
                "OSNAME" => detail.os_name = entry[1].into(),
                "NOFORTRAN" => detail.no_fortran = true,
                "CEXTRALIB" => detail.c_extra_libs = LinkFlags::parse(entry[1])?,
                "FEXTRALIB" => detail.f_extra_libs = LinkFlags::parse(entry[1])?,
                _ => continue,
            }
        }
        Ok(detail)
    }
}

/// Library inspection using binutils (`nm` and `objdump`) as external command
///
/// - Linked shared libraries using `objdump -p` external command.
/// - Global "T" symbols in the text (code) section of library using `nm -g` external command.
#[derive(Debug, Clone)]
pub struct LibInspect {
    path: PathBuf,
    pub libs: Vec<String>,
    pub symbols: Vec<String>,
}

impl LibInspect {
    /// Inspect library file
    ///
    /// Be sure that `nm -g` and `objdump -p` are executed in this function
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::LibraryNotExist {
                path: path.to_owned(),
            });
        }

        let nm_out = Command::new("nm").arg("-g").arg(path).output()?;

        // assumes `nm` output like following:
        //
        // ```
        // 0000000000909b30 T zupmtr_
        // ```
        let mut symbols: Vec<_> = nm_out
            .stdout
            .lines()
            .flat_map(|line| {
                let line = line.expect("nm output should not include non-UTF8 output");
                let entry: Vec<_> = line.trim().split(" ").collect();
                if entry.len() == 3 && entry[1] == "T" {
                    Some(entry[2].into())
                } else {
                    None
                }
            })
            .collect();
        symbols.sort(); // sort alphabetically

        let mut libs: Vec<_> = Command::new("objdump")
            .arg("-p")
            .arg(path)
            .output()?
            .stdout
            .lines()
            .flat_map(|line| {
                let line = line.expect("objdump output should not include non-UTF8 output");
                if line.trim().starts_with("NEEDED") {
                    Some(line.trim().trim_start_matches("NEEDED").trim().to_string())
                } else {
                    None
                }
            })
            .collect();
        libs.sort();

        Ok(LibInspect {
            path: path.into(),
            libs,
            symbols,
        })
    }

    pub fn has_cblas(&self) -> bool {
        for sym in &self.symbols {
            if sym.starts_with("cblas_") {
                return true;
            }
        }
        return false;
    }

    pub fn has_lapack(&self) -> bool {
        for sym in &self.symbols {
            if sym == "dsyev_" {
                return true;
            }
        }
        return false;
    }

    pub fn has_lapacke(&self) -> bool {
        for sym in &self.symbols {
            if sym.starts_with("LAPACKE_") {
                return true;
            }
        }
        return false;
    }

    pub fn has_lib(&self, name: &str) -> bool {
        for lib in &self.libs {
            if let Some(stem) = lib.split(".").next() {
                if stem == format!("lib{}", name) {
                    return true;
                }
            };
        }
        return false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_from_makefile_conf() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Makefile.conf");
        assert!(path.exists());
        let detail = MakeConf::new(path).unwrap();
        assert!(!detail.no_fortran);
    }

    #[test]
    fn detail_from_nofortran_conf() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("nofortran.conf");
        assert!(path.exists());
        let detail = MakeConf::new(path).unwrap();
        assert!(detail.no_fortran);
    }
}
