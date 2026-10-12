//! Read the exact resolved DirectWrite face through its own file loader.
//! System font bytes stay in memory; only identity/size metadata is logged.

use super::super::{Com, com_ok, family_name, ffi, font_table};
use std::ffi::c_void;

#[repr(C)]
struct FaceFiles {
    _slots_0_3: [usize; 4],
    files: unsafe extern "system" fn(*mut ffi::Face, *mut u32, *mut *mut File) -> i32,
    index: unsafe extern "system" fn(*mut ffi::Face) -> u32,
    simulations: unsafe extern "system" fn(*mut ffi::Face) -> u32,
}
#[repr(C)]
struct File {
    vtbl: *const FileVtbl,
}
#[repr(C)]
struct FileVtbl {
    _slots_0_2: [usize; 3],
    key: unsafe extern "system" fn(*mut File, *mut *const c_void, *mut u32) -> i32,
    loader: unsafe extern "system" fn(*mut File, *mut *mut Loader) -> i32,
}
#[repr(C)]
struct Loader {
    vtbl: *const LoaderVtbl,
}
#[repr(C)]
struct LoaderVtbl {
    _slots_0_2: [usize; 3],
    stream: unsafe extern "system" fn(*mut Loader, *const c_void, u32, *mut *mut Stream) -> i32,
}
#[repr(C)]
struct Stream {
    vtbl: *const StreamVtbl,
}
#[repr(C)]
struct StreamVtbl {
    _slots_0_2: [usize; 3],
    read: unsafe extern "system" fn(
        *mut Stream,
        *mut *const c_void,
        u64,
        u64,
        *mut *mut c_void,
    ) -> i32,
    release: unsafe extern "system" fn(*mut Stream, *mut c_void),
    size: unsafe extern "system" fn(*mut Stream, *mut u64) -> i32,
}

struct Fragment<'a> {
    stream: &'a Com<Stream>,
    context: *mut c_void,
}
impl Drop for Fragment<'_> {
    fn drop(&mut self) {
        // SAFETY: ReadFileFragment succeeded on this retained stream. Even a
        // null context must be released; pointer presence is not ownership.
        unsafe {
            let table = &*(*self.stream.as_ptr()).vtbl;
            (table.release)(self.stream.as_ptr(), self.context);
        }
    }
}

fn read_file(file: &Com<File>) -> Vec<u8> {
    let mut key = std::ptr::null();
    let mut key_size = 0;
    let mut loader = std::ptr::null_mut();
    let mut stream = std::ptr::null_mut();
    // SAFETY: pinned dwrite.h File slots 3/4, Loader slot 3, Stream slots 3..5.
    // COM owners retain the key, loader and stream across dependent calls.
    unsafe {
        let table = &*(*file.as_ptr()).vtbl;
        assert!(com_ok((table.key)(
            file.as_ptr(),
            &raw mut key,
            &raw mut key_size
        )));
        assert!(!key.is_null() && key_size > 0);
        assert!(com_ok((table.loader)(file.as_ptr(), &raw mut loader)));
        let loader = Com::from_raw(loader).unwrap();
        assert!(com_ok(((*(*loader.as_ptr()).vtbl).stream)(
            loader.as_ptr(),
            key,
            key_size,
            &raw mut stream,
        )));
        let stream = Com::from_raw(stream).unwrap();
        let table = &*(*stream.as_ptr()).vtbl;
        let mut size = 0;
        assert!(com_ok((table.size)(stream.as_ptr(), &raw mut size)));
        // Bound this experiment explicitly; oversized/multi-file faces are
        // unsupported here, never silently reconstructed from selected tables.
        assert!((1..=64 * 1024 * 1024).contains(&size));
        let mut data = std::ptr::null();
        let mut context = std::ptr::null_mut();
        assert!(com_ok((table.read)(
            stream.as_ptr(),
            &raw mut data,
            0,
            size,
            &raw mut context,
        )));
        let _lease = Fragment {
            stream: &stream,
            context,
        };
        assert!(!data.is_null());
        std::slice::from_raw_parts(data.cast::<u8>(), usize::try_from(size).unwrap()).to_vec()
    }
}

fn dword(bytes: &[u8], at: usize) -> usize {
    usize::try_from(u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())).unwrap()
}

// Compare every SFNT table, including collection face offsets, with the exact
// resolved native face. A family-name lookup cannot establish this identity.
fn check_tables(face: &Com<ffi::Face>, bytes: &[u8], index: u32) -> usize {
    let offset = if bytes.starts_with(b"ttcf") {
        assert!(usize::try_from(index).unwrap() < dword(bytes, 8));
        dword(bytes, 12 + usize::try_from(index).unwrap() * 4)
    } else {
        assert_eq!(index, 0);
        0
    };
    let count = super::super::word(bytes, offset + 4);
    for entry in 0..count {
        let at = offset + 12 + entry * 16;
        let tag = bytes[at..at + 4].try_into().unwrap();
        let start = dword(bytes, at + 8);
        let length = dword(bytes, at + 12);
        assert_eq!(
            font_table(face, tag).as_deref(),
            Some(&bytes[start..start + length]),
            "table {tag:?}"
        );
    }
    count
}

pub(super) fn exact_face(face: &Com<ffi::Face>) -> harfrust::Font {
    let mut count = 0;
    let mut file = std::ptr::null_mut();
    // SAFETY: pinned IDWriteFontFace slots 4..6; every returned file is adopted
    // once. This prototype supports only a single static, unsimulated face.
    let (file, index) = unsafe {
        let table = &**face.as_ptr().cast::<*const FaceFiles>();
        assert!(com_ok((table.files)(
            face.as_ptr(),
            &raw mut count,
            std::ptr::null_mut()
        )));
        assert_eq!(count, 1, "multi-file faces are outside this experiment");
        assert_eq!((table.simulations)(face.as_ptr()), 0, "simulated face");
        assert!(
            font_table(face, *b"fvar").is_none(),
            "variable face needs exact axes"
        );
        assert!(com_ok((table.files)(
            face.as_ptr(),
            &raw mut count,
            &raw mut file
        )));
        let file = Com::from_raw(file).unwrap();
        assert_eq!(count, 1);
        (file, (table.index)(face.as_ptr()))
    };
    let bytes = read_file(&file);
    let tables = check_tables(face, &bytes, index);
    let family = family_name(&font_table(face, *b"name").unwrap()).unwrap();
    eprintln!(
        "full-source exact face: family={family:?}, index={index}, bytes={}, tables={tables}, simulations=0, variable=false",
        bytes.len()
    );
    harfrust::Font::new(bytes, index)
        .unwrap()
        .instance_builder()
        .build()
}
