//! The parts of `std` that differ between the desktop and the browser.
//!
//! On the desktop each is a thin pass-through. In the browser there is no
//! disk, so `fs` is an in-memory tree the page fills with the player's own
//! copy of the game, and a "thread" runs its job on the spot.

pub mod fs {
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};

    /// One entry of a folder listing.
    pub struct DirEntry {
        path: PathBuf,
    }

    impl DirEntry {
        pub fn path(&self) -> PathBuf {
            self.path.clone()
        }

        pub fn file_name(&self) -> OsString {
            self.path.file_name().map(OsString::from).unwrap_or_default()
        }
    }

    pub type ReadDir = std::vec::IntoIter<io::Result<DirEntry>>;

    #[cfg(not(target_arch = "wasm32"))]
    mod imp {
        use super::*;

        pub use std::fs::File;

        pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
            std::fs::read(path)
        }

        pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
            std::fs::read_to_string(path)
        }

        pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> io::Result<()> {
            std::fs::write(path, data)
        }

        pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
            std::fs::create_dir_all(path)
        }

        pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
            Ok(std::fs::read_dir(path)?
                .map(|e| e.map(|e| DirEntry { path: e.path() }))
                .collect::<Vec<_>>()
                .into_iter())
        }

        pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
            std::fs::remove_file(path)
        }

        pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
            std::fs::remove_dir_all(path)
        }

        pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
            std::fs::copy(from, to)
        }

        pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
            File::create(path)
        }

        pub fn open(path: impl AsRef<Path>) -> io::Result<io::Cursor<Vec<u8>>> {
            read(path).map(io::Cursor::new)
        }

        pub fn is_file(path: impl AsRef<Path>) -> bool {
            path.as_ref().is_file()
        }

        pub fn is_dir(path: impl AsRef<Path>) -> bool {
            path.as_ref().is_dir()
        }

        pub fn exists(path: impl AsRef<Path>) -> bool {
            path.as_ref().exists()
        }

        pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
            path.as_ref().canonicalize()
        }

        /// Everything is on the disk already.
        pub fn ensure_folder(_: impl AsRef<Path>) -> bool {
            true
        }
    }

    #[cfg(target_arch = "wasm32")]
    mod imp {
        use super::*;
        use std::collections::{BTreeMap, BTreeSet};
        use std::sync::Mutex;

        /// A file's path as given and its bytes. A "remote" file is known
        /// to exist (it is listed, with its size) but its bytes are not
        /// here until the page has fetched them.
        struct Entry {
            orig: String,
            data: Vec<u8>,
            loaded: bool,
        }

        /// Files by lower-cased path (the game's files are matched without
        /// regard to case, as Windows does).
        struct Tree {
            files: BTreeMap<String, Entry>,
            dirs: BTreeSet<String>,
            /// Folders the page has been asked to fetch.
            asked: BTreeSet<String>,
        }

        static TREE: Mutex<Tree> = Mutex::new(Tree {
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
            asked: BTreeSet::new(),
        });

        fn tree() -> std::sync::MutexGuard<'static, Tree> {
            TREE.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// A path as `a/b/c`: forward slashes, no `.` parts, no leading or
        /// trailing slash.
        fn norm(path: &Path) -> String {
            path.to_string_lossy()
                .replace('\\', "/")
                .split('/')
                .filter(|p| !p.is_empty() && *p != ".")
                .collect::<Vec<_>>()
                .join("/")
        }

        fn key(path: &Path) -> String {
            norm(path).to_ascii_lowercase()
        }

        fn not_found(path: &Path) -> io::Error {
            io::Error::new(io::ErrorKind::NotFound, format!("{}: not found", path.display()))
        }

        fn put(path: &Path, data: Vec<u8>, loaded: bool) {
            let mut t = tree();
            let mut parent = key(path);
            while let Some(i) = parent.rfind('/') {
                parent.truncate(i);
                if !t.dirs.insert(parent.clone()) {
                    break;
                }
            }
            t.files.insert(key(path), Entry { orig: norm(path), data, loaded });
        }

        /// What the player saves lives under this folder (see
        /// `profile::user_dir`), and the page keeps it between visits.
        const SAVED: &str = "mm2-user";

        fn saved(path: &Path) -> bool {
            key(path).split('/').next() == Some(SAVED)
        }

        /// Puts a file in the tree, along with the folders above it.
        pub fn insert(path: &Path, data: Vec<u8>) {
            if saved(path) {
                mm2_persist(&norm(path), &data);
            }
            put(path, data, true);
        }

        /// A saved file the page kept from an earlier visit; it is not
        /// handed back to the page.
        pub fn restore(path: &Path, data: Vec<u8>) {
            put(path, data, true);
        }

        /// Lists a file whose bytes the page will fetch when asked.
        pub fn insert_remote(path: &Path) {
            put(path, Vec::new(), false);
        }

        /// Whether every file under `dir` is here. If not, asks the page
        /// (once) to fetch the missing ones, and lets go of the bytes of
        /// the other folders that were fetched this way, to free memory.
        /// `fetch` is told the folder and the paths wanted.
        pub fn ensure(dir: impl AsRef<Path>, fetch: impl FnOnce(&str, Vec<String>)) -> bool {
            let dir = key(dir.as_ref());
            let prefix = format!("{dir}/");
            let mut t = tree();
            let missing: Vec<String> = t
                .files
                .iter()
                .filter(|(k, e)| k.starts_with(&prefix) && !e.loaded)
                .map(|(_, e)| e.orig.clone())
                .collect();
            if missing.is_empty() {
                return true;
            }
            if t.asked.insert(dir.clone()) {
                // Folders asked for before this one, and not this one's.
                let others: Vec<String> = t.asked.iter().filter(|d| **d != dir).cloned().collect();
                for d in others {
                    let p = format!("{d}/");
                    for (k, e) in t.files.iter_mut() {
                        if k.starts_with(&p) && e.loaded {
                            e.data = Vec::new();
                            e.loaded = false;
                        }
                    }
                    t.asked.remove(&d);
                }
                drop(t);
                fetch(&dir, missing);
            }
            false
        }

        #[wasm_bindgen::prelude::wasm_bindgen]
        extern "C" {
            /// The page keeps what the player saves: this gives it a file's
            /// bytes (`mm2Persist`) or tells it a file or folder is gone.
            #[wasm_bindgen(js_name = mm2Persist)]
            fn mm2_persist(path: &str, data: &[u8]);
            #[wasm_bindgen(js_name = mm2Forget)]
            fn mm2_forget(path: &str, folder: bool);
            /// The page's fetcher (index.html): gets the files, one path to
            /// a line, relative to the game folder, and adds them.
            #[wasm_bindgen(js_name = mm2Fetch)]
            fn mm2_fetch(folder: &str, paths: &str);
        }

        /// The page could not fetch a folder's files: forgets that it was
        /// asked, so the next `ensure_folder` asks again.
        pub fn fetch_failed(dir: impl AsRef<Path>) {
            tree().asked.remove(&key(dir.as_ref()));
        }

        /// Whether the folder's files are here, asking the page for them
        /// if not; the caller tries again on a later frame.
        pub fn ensure_folder(dir: impl AsRef<Path>) -> bool {
            ensure(dir, |folder, paths| {
                let rel: Vec<&str> = paths.iter().map(|p| p.split_once('/').map_or(&p[..], |x| x.1)).collect();
                mm2_fetch(folder, &rel.join("
"));
            })
        }

        pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
            let path = path.as_ref();
            tree()
                .files
                .get(&key(path))
                .filter(|e| e.loaded)
                .map(|e| e.data.clone())
                .ok_or_else(|| not_found(path))
        }

        pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
            String::from_utf8(read(path)?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }

        pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> io::Result<()> {
            insert(path.as_ref(), data.as_ref().to_vec());
            Ok(())
        }

        pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
            let mut t = tree();
            let mut dir = key(path.as_ref());
            while !dir.is_empty() && t.dirs.insert(dir.clone()) {
                match dir.rfind('/') {
                    Some(i) => dir.truncate(i),
                    None => break,
                }
            }
            Ok(())
        }

        pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
            let path = path.as_ref();
            let dir = key(path);
            let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
            let t = tree();
            if !dir.is_empty() && !t.dirs.contains(&dir) {
                return Err(not_found(path));
            }
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for (k, Entry { orig, .. }) in t.files.range(prefix.clone()..) {
                if !k.starts_with(&prefix) {
                    break;
                }
                // Lower-casing is ASCII only, so `k` and `orig` have the same
                // byte offsets.
                let rest = &k[prefix.len()..];
                let name = rest.split('/').next().unwrap_or(rest);
                if seen.insert(name.to_string()) {
                    let end = prefix.len() + name.len();
                    out.push(Ok(DirEntry { path: PathBuf::from(&orig[..end]) }));
                }
            }
            Ok(out.into_iter())
        }

        pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
            let path = path.as_ref();
            if saved(path) {
                mm2_forget(&norm(path), false);
            }
            tree().files.remove(&key(path)).map(|_| ()).ok_or_else(|| not_found(path))
        }

        pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
            if saved(path.as_ref()) {
                mm2_forget(&norm(path.as_ref()), true);
            }
            let dir = key(path.as_ref());
            let prefix = format!("{dir}/");
            let mut t = tree();
            t.files.retain(|k, _| !k.starts_with(&prefix));
            t.dirs.retain(|d| *d != dir && !d.starts_with(&prefix));
            Ok(())
        }

        pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
            let data = read(from)?;
            let n = data.len() as u64;
            write(to, data)?;
            Ok(n)
        }

        pub fn open(path: impl AsRef<Path>) -> io::Result<io::Cursor<Vec<u8>>> {
            read(path).map(io::Cursor::new)
        }

        pub fn is_file(path: impl AsRef<Path>) -> bool {
            tree().files.contains_key(&key(path.as_ref()))
        }

        pub fn is_dir(path: impl AsRef<Path>) -> bool {
            tree().dirs.contains(&key(path.as_ref()))
        }

        pub fn exists(path: impl AsRef<Path>) -> bool {
            is_file(path.as_ref()) || is_dir(path.as_ref())
        }

        pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
            let path = path.as_ref();
            if exists(path) {
                Ok(PathBuf::from(norm(path)))
            } else {
                Err(not_found(path))
            }
        }

        /// A file being written; it joins the tree when flushed or dropped.
        pub struct File {
            path: PathBuf,
            data: Vec<u8>,
        }

        impl io::Write for File {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.data.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                insert(&self.path, self.data.clone());
                Ok(())
            }
        }

        impl Drop for File {
            fn drop(&mut self) {
                insert(&self.path, std::mem::take(&mut self.data));
            }
        }

        pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
            Ok(File { path: path.as_ref().to_path_buf(), data: Vec::new() })
        }
    }

    pub use imp::*;
}

/// The browser build fills the window behind the menus: their 640×480
/// stays undistorted, and a menu's background picture is carried on past
/// its edges to the window's (`Sprites::backdrop`).
pub const FILL_MENUS: bool = cfg!(target_arch = "wasm32");

/// The browser build has no display modes to choose: the game fills the
/// page, so the Graphics page's Display Resolution list shows Auto, greyed.
pub const DISPLAY_MODES: bool = !cfg!(target_arch = "wasm32");

/// The browser build notes under the Graphics page's quality slider what it
/// does here, set the view distance; the desktop shows the page as it is.
pub const VIEW_DISTANCE_NOTE: bool = cfg!(target_arch = "wasm32");

static SCREEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Notes the size of the window's drawing surface, in pixels.
pub fn set_screen(width: u32, height: u32) {
    SCREEN.store(((width as u64) << 32) | height as u64, std::sync::atomic::Ordering::Relaxed);
}

/// The window's drawing surface, in pixels, as last noted (0 by 0 before
/// the first).
pub fn screen() -> (u32, u32) {
    let v = SCREEN.load(std::sync::atomic::Ordering::Relaxed);
    ((v >> 32) as u32, v as u32)
}

/// Leaves the game for the page's start screen (the X on the main menu); on
/// the desktop the caller closes the window instead.
#[cfg(target_arch = "wasm32")]
pub fn quit() {
    #[wasm_bindgen::prelude::wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = mm2Quit)]
        fn quit();
    }
    quit();
}
#[cfg(not(target_arch = "wasm32"))]
pub fn quit() {}

pub mod thread {
    use std::future::Future;
    use std::time::Duration;

    /// A job started by [`spawn_async`].
    #[cfg(not(target_arch = "wasm32"))]
    pub struct Job<T>(std::thread::JoinHandle<T>);

    #[cfg(not(target_arch = "wasm32"))]
    impl<T> Job<T> {
        pub fn is_finished(&self) -> bool {
            self.0.is_finished()
        }

        pub fn join(self) -> std::thread::Result<T> {
            self.0.join()
        }
    }

    /// Runs what `f` makes on a thread of its own.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_async<F, Fut, T>(f: F) -> Job<T>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        Job(std::thread::spawn(move || pollster::block_on(f())))
    }

    /// The browser's page has one thread, so the job runs on it, a stretch
    /// at a time: it gives the page back at each `yield_now`.
    #[cfg(target_arch = "wasm32")]
    pub struct Job<T>(std::rc::Rc<std::cell::RefCell<Option<T>>>);

    #[cfg(target_arch = "wasm32")]
    impl<T> Job<T> {
        pub fn is_finished(&self) -> bool {
            self.0.borrow().is_some()
        }

        pub fn join(self) -> std::thread::Result<T> {
            Ok(self.0.borrow_mut().take().expect("joined before it finished"))
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn spawn_async<F, Fut, T>(f: F) -> Job<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T> + 'static,
        T: 'static,
    {
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let out = slot.clone();
        let fut = f();
        #[wasm_bindgen::prelude::wasm_bindgen]
        extern "C" {
            /// The page swaps the pointer for the game's hourglass.
            #[wasm_bindgen(js_name = mm2Busy)]
            fn busy(on: bool);
        }
        busy(true);
        wasm_bindgen_futures::spawn_local(async move {
            // The click that started the job is still sounding, from this
            // same thread: let it finish before the work takes the thread.
            pause(350).await;
            let v = fut.await;
            *out.borrow_mut() = Some(v);
            busy(false);
        });
        Job(slot)
    }

    /// Lets the page draw and answer before the job goes on: it waits for
    /// the next frame to be drawn (and for a timer too, in case the page is
    /// hidden and has no frames). A job on a thread of its own has no need
    /// to.
    pub async fn yield_now() {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::{closure::Closure, JsCast};
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                let Some(w) = web_sys::window() else { return };
                // After the frame's own callbacks (the game's drawing is
                // one), not inside them.
                let late = resolve.clone();
                let after_frame = Closure::once_into_js(move || {
                    if let Some(w) = web_sys::window() {
                        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&late, 0);
                    }
                });
                let _ = w.request_animation_frame(after_frame.unchecked_ref());
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 150);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }

    /// Waits `ms` milliseconds, in the browser.
    #[cfg(target_arch = "wasm32")]
    pub async fn pause(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            }
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// Waits; in the browser there is nothing to wait on, so it does not.
    pub fn sleep(d: Duration) {
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(d);
        #[cfg(target_arch = "wasm32")]
        let _ = d;
    }
}

/// A line for whoever is watching: stderr, or the browser's console.
pub fn log(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::warn_1(&msg.into());
    #[cfg(not(target_arch = "wasm32"))]
    std::eprintln!("{msg}");
}
