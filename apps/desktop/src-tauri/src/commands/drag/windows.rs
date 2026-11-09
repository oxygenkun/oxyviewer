//! Windows file transfer without drag 2.1.1's canonicalize -> PIDL conversion.
//! That conversion gives UNC/mapped paths a verbatim prefix the Shell rejects.
use std::{
    mem::{ManuallyDrop, size_of},
    os::windows::ffi::OsStrExt,
    path::PathBuf,
};
use windows::{
    Win32::{
        Foundation::{COLORREF, GlobalFree, SIZE},
        Graphics::{
            Gdi::{CreateBitmap, DeleteObject},
            Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
                WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
            },
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject,
                STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
            },
            Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{CF_HDROP, DROPEFFECT_COPY, OleInitialize, OleUninitialize},
        },
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
            Shell::{
                CLSID_DragDropHelper, DROPFILES, IDragSourceHelper, SHCreateDataObject,
                SHDRAGIMAGE, SHDoDragDrop,
            },
        },
    },
    core::{BOOL, Error, Result},
};

struct OleApartment;

impl OleApartment {
    fn new() -> Result<Self> {
        // SAFETY: Called on the Tauri UI thread (or a dedicated test thread).
        // Both S_OK and S_FALSE require a matching OleUninitialize.
        unsafe { OleInitialize(None)? };
        Ok(Self)
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        // SAFETY: Balances this thread's successful OleInitialize call.
        unsafe { OleUninitialize() };
    }
}

fn file_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    }
}

fn file_data_object(paths: &[PathBuf]) -> Result<IDataObject> {
    let mut names = Vec::<u16>::new();
    for path in paths {
        names.extend(path.as_os_str().encode_wide());
        names.push(0);
    }
    names.push(0);
    // SAFETY: The apartment is initialized by the caller. All pointers below
    // refer to owned, correctly sized storage and are used only synchronously.
    unsafe {
        let object: IDataObject = SHCreateDataObject(None, None, None)?;
        let memory = GlobalAlloc(
            GMEM_MOVEABLE | GMEM_ZEROINIT,
            size_of::<DROPFILES>() + names.len() * size_of::<u16>(),
        )?;
        let buffer = GlobalLock(memory);
        if buffer.is_null() {
            let error = Error::from_win32();
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
        buffer.cast::<DROPFILES>().write(DROPFILES {
            pFiles: size_of::<DROPFILES>() as u32,
            fWide: BOOL(1),
            ..Default::default()
        });
        std::ptr::copy_nonoverlapping(
            names.as_ptr(),
            buffer.add(size_of::<DROPFILES>()).cast::<u16>(),
            names.len(),
        );
        // A zero return is normal when the lock count reaches zero.
        let _ = GlobalUnlock(memory);
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: memory },
            pUnkForRelease: ManuallyDrop::new(None),
        };
        // Transfer ownership only on success; no file reads or mutations here.
        if let Err(error) = object.SetData(&file_format(), &medium, true) {
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
        Ok(object)
    }
}

fn set_drag_image(object: &IDataObject, png: &[u8]) -> Result<()> {
    // SAFETY: COM is initialized; the small prepared preview and all pixel
    // buffers stay alive throughout their synchronous native calls.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(png)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            std::ptr::null(),
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        let source = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &frame)?;
        let mut pixels = vec![0; (width * height * 4) as usize];
        source.CopyPixels(std::ptr::null(), width * 4, &mut pixels)?;
        let bitmap = CreateBitmap(
            width as i32,
            height as i32,
            1,
            32,
            Some(pixels.as_ptr().cast()),
        );
        if bitmap.is_invalid() {
            return Err(Error::from_win32());
        }
        let result = (|| {
            let helper: IDragSourceHelper =
                CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)?;
            helper.InitializeFromBitmap(
                &SHDRAGIMAGE {
                    sizeDragImage: SIZE {
                        cx: width as i32,
                        cy: height as i32,
                    },
                    hbmpDragImage: bitmap,
                    crColorKey: COLORREF(0xFFFF_FFFF),
                    ..Default::default()
                },
                object,
            )
        })();
        // InitializeFromBitmap copies the bitmap into the data object.
        let _ = DeleteObject(bitmap.into());
        result
    }
}

pub(super) fn start_drag(window: &tauri::Window, paths: &[PathBuf], image: &[u8]) -> Result<()> {
    let _apartment = OleApartment::new()?;
    // SAFETY: Read only the current button state. Slow-volume validation may
    // have outlived the gesture; never start a new drag after button release.
    if unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) } >= 0 {
        return Ok(());
    }
    let object = file_data_object(paths)?;
    // Feedback failure must not stop transfer of the original files.
    let _ = set_drag_image(&object, image);
    // SAFETY: HWND and the data object belong to this UI thread. The Shell
    // supplies IDropSource; this call returns only after drop or cancellation.
    unsafe { SHDoDragDrop(window.hwnd().ok(), &object, None, DROPEFFECT_COPY)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        System::{Com::DATADIR_GET, Ole::ReleaseStgMedium},
        UI::Shell::{DragQueryFileW, HDROP},
    };

    #[test]
    fn transfers_shell_readable_original_paths_without_resolving_network_volumes() {
        let _apartment = OleApartment::new().unwrap();
        let paths = [
            r"\\nas\photos\照片 one.HIF",
            r"W:\photos\two.NEF",
            r"C:\photos\three.png",
            r"\\nas\photos\four.jpg",
            r"\\?\C:\photos\verbatim-only.jpg.",
        ]
        .map(PathBuf::from);
        let expected = [
            r"\\nas\photos\照片 one.HIF",
            r"W:\photos\two.NEF",
            r"C:\photos\three.png",
            r"\\nas\photos\four.jpg",
            r"\\?\C:\photos\verbatim-only.jpg.",
        ];
        let object = file_data_object(&paths).unwrap();
        // SAFETY: COM is initialized and GetData returns an owned HGLOBAL.
        // Read through the same Shell API used by drop targets, then release it.
        unsafe {
            object.QueryGetData(&file_format()).ok().unwrap();
            let formats = object.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
            let mut found = false;
            let mut next = [FORMATETC::default()];
            while formats.Next(&mut next, None).0 == 0 {
                if next[0].cfFormat == CF_HDROP.0 {
                    found = true;
                    break;
                }
            }
            assert!(found, "drop targets must be able to discover CF_HDROP");
            let mut medium = object.GetData(&file_format()).unwrap();
            let drop = HDROP(medium.u.hGlobal.0);
            assert_eq!(DragQueryFileW(drop, u32::MAX, None), expected.len() as u32);
            for (index, expected) in expected.iter().enumerate() {
                let size = DragQueryFileW(drop, index as u32, None);
                let mut name = vec![0; size as usize + 1];
                assert_eq!(DragQueryFileW(drop, index as u32, Some(&mut name)), size);
                assert_eq!(
                    String::from_utf16(&name[..size as usize]).unwrap(),
                    *expected
                );
            }
            ReleaseStgMedium(&mut medium);
        }
    }
}
