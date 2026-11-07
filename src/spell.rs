//! Spell checking with Hunspell dictionaries (via spellbook).
//!
//! Dictionaries come from the system (`/usr/share/hunspell`, also the host's
//! inside the Flatpak sandbox), the app's data directory, or are downloaded
//! once into the data directory from the wooorm/dictionaries collection.
//! Loading runs on a thread; until a dictionary is ready nothing is marked.
//! Words added by the user go to a personal word list per language.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use spellbook::Dictionary;

/// Language code (as in `TextData::lang`) → (Hunspell file stem, wooorm folder).
const DICTS: &[(&str, &str, &str)] = &[
    ("en-us", "en_US", "en"),
    ("en-gb", "en_GB", "en-GB"),
    ("pl", "pl_PL", "pl"),
    ("de-1996", "de_DE", "de"),
    ("fr", "fr_FR", "fr"),
    ("es", "es_ES", "es"),
    ("it", "it_IT", "it"),
    ("pt", "pt_PT", "pt"),
    ("nl", "nl_NL", "nl"),
    ("sv", "sv_SE", "sv"),
    ("da", "da_DK", "da"),
    ("nb", "nb_NO", "nb"),
    ("cs", "cs_CZ", "cs"),
    ("sk", "sk_SK", "sk"),
    ("hu", "hu_HU", "hu"),
    ("ro", "ro_RO", "ro"),
    ("ru", "ru_RU", "ru"),
    ("uk", "uk_UA", "uk"),
    ("tr", "tr_TR", "tr"),
    ("el-monoton", "el_GR", "el"),
];

enum State {
    Loading(Receiver<Result<Dictionary, String>>),
    Ready(Arc<Dictionary>),
    Failed,
}

#[derive(Default)]
pub struct SpellChecker {
    dicts: HashMap<String, State>,
    /// Per language: word → correct.
    memo: HashMap<String, HashMap<String, bool>>,
    personal: HashMap<String, HashSet<String>>,
    /// Words ignored for this session.
    ignored: HashSet<String>,
}

fn data_dir() -> PathBuf {
    crate::fonts::data_dir().join("dictionaries")
}

/// Whether this process may touch the network and user data (not in tests or
/// headless runs).
fn allowed() -> bool {
    !cfg!(test) && crate::recent::ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn find_local(stem: &str, folder: &str) -> Option<(PathBuf, PathBuf)> {
    let mut dirs: Vec<PathBuf> = ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts", "/run/host/usr/share/hunspell", "/run/host/usr/share/myspell", "/app/share/hunspell"].iter().map(PathBuf::from).collect();
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("hunspell"));
    }
    for d in &dirs {
        let (aff, dic) = (d.join(format!("{stem}.aff")), d.join(format!("{stem}.dic")));
        if aff.exists() && dic.exists() {
            return Some((aff, dic));
        }
    }
    let d = data_dir().join(folder);
    let (aff, dic) = (d.join("index.aff"), d.join("index.dic"));
    (aff.exists() && dic.exists()).then_some((aff, dic))
}

fn load(stem: &str, folder: &str, download: bool) -> Result<Dictionary, String> {
    let (aff, dic) = match find_local(stem, folder) {
        Some(p) => p,
        None if download => {
            let base = format!("https://raw.githubusercontent.com/wooorm/dictionaries/main/dictionaries/{folder}");
            let aff = crate::fonts::http_get(&format!("{base}/index.aff"))?;
            let dic = crate::fonts::http_get(&format!("{base}/index.dic"))?;
            let d = data_dir().join(folder);
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            std::fs::write(d.join("index.aff"), aff).map_err(|e| e.to_string())?;
            std::fs::write(d.join("index.dic"), dic).map_err(|e| e.to_string())?;
            (d.join("index.aff"), d.join("index.dic"))
        }
        None => return Err("no dictionary".into()),
    };
    let read = |p: &PathBuf| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string());
    Dictionary::new(&read(&aff)?, &read(&dic)?).map_err(|e| e.to_string())
}

fn personal_file(lang: &str) -> PathBuf {
    data_dir().join(format!("personal-{lang}.txt"))
}

/// Words of `text` worth checking: letters (and inner apostrophes),
/// skipping numbers, acronyms and mixed-case identifiers.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let inner_apos = (c == '\'' || c == '’') && start.is_some() && chars.get(k + 1).is_some_and(|n| n.1.is_alphabetic());
        if c.is_alphabetic() || inner_apos || (start.is_some() && c.is_ascii_digit()) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push(s..i);
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out.retain(|r| {
        let w = &text[r.clone()];
        let n = w.chars().count();
        n > 1 && !w.chars().any(|c| c.is_ascii_digit()) && !w.chars().skip(1).any(|c| c.is_uppercase())
    });
    out
}

impl SpellChecker {
    /// The dictionary for `lang`, starting to load it if needed. `None`
    /// while loading or when unavailable.
    fn dict(&mut self, lang: &str) -> Option<Arc<Dictionary>> {
        let lang = lang.to_lowercase();
        if !self.dicts.contains_key(&lang) {
            let Some(&(_, stem, folder)) = DICTS.iter().find(|d| d.0 == lang) else {
                self.dicts.insert(lang, State::Failed);
                return None;
            };
            if !allowed() {
                self.dicts.insert(lang, State::Failed);
                return None;
            }
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(load(stem, folder, true));
            });
            self.dicts.insert(lang.clone(), State::Loading(rx));
            self.personal.insert(lang.clone(), std::fs::read_to_string(personal_file(&lang)).unwrap_or_default().lines().map(str::to_string).collect());
        }
        let st = self.dicts.get_mut(&lang)?;
        if let State::Loading(rx) = st {
            match rx.try_recv() {
                Ok(Ok(d)) => *st = State::Ready(Arc::new(d)),
                Ok(Err(e)) => {
                    eprintln!("spelling dictionary {lang}: {e}");
                    *st = State::Failed;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => *st = State::Failed,
            }
        }
        match st {
            State::Ready(d) => Some(d.clone()),
            _ => None,
        }
    }

    /// Whether a dictionary for `lang` is still loading (repaint until it is).
    pub fn loading(&self, lang: &str) -> bool {
        matches!(self.dicts.get(&lang.to_lowercase()), Some(State::Loading(_)))
    }

    /// Byte ranges of misspelled words in `text`.
    pub fn misspelled(&mut self, text: &str, lang: &str) -> Vec<Range<usize>> {
        let Some(d) = self.dict(lang) else { return Vec::new() };
        let lang = lang.to_lowercase();
        let personal = self.personal.get(&lang).cloned().unwrap_or_default();
        let memo = self.memo.entry(lang).or_default();
        words(text)
            .into_iter()
            .filter(|r| {
                let w = text[r.clone()].replace('’', "'");
                if self.ignored.contains(&w) || personal.contains(&w) || personal.contains(&w.to_lowercase()) {
                    return false;
                }
                !*memo.entry(w.clone()).or_insert_with(|| d.check(&w))
            })
            .collect()
    }

    pub fn suggest(&mut self, word: &str, lang: &str) -> Vec<String> {
        let Some(d) = self.dict(lang) else { return Vec::new() };
        let mut out = Vec::new();
        d.suggest(&word.replace('’', "'"), &mut out);
        out.truncate(6);
        out
    }

    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(word.replace('’', "'"));
    }

    /// Add to the personal word list of `lang` (saved in the data dir).
    pub fn learn(&mut self, word: &str, lang: &str) {
        let lang = lang.to_lowercase();
        let w = word.replace('’', "'");
        let set = self.personal.entry(lang.clone()).or_default();
        set.insert(w);
        if allowed() {
            let mut list: Vec<&String> = set.iter().collect();
            list.sort();
            let body: String = list.into_iter().map(|w| format!("{w}\n")).collect();
            let _ = std::fs::create_dir_all(data_dir());
            let _ = std::fs::write(personal_file(&lang), body);
        }
    }

    /// Check with an already loaded dictionary (tests).
    #[cfg(test)]
    pub fn with_dictionary(lang: &str, d: Dictionary) -> Self {
        let mut s = Self::default();
        s.dicts.insert(lang.into(), State::Ready(Arc::new(d)));
        s
    }
}
