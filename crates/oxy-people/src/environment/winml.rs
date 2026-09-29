//! Windows ML 2.4.89 catalog C ABI (WinMLEpCatalog.h / WinMLAsync.h).
//! Only the verified runtime is loaded; Windows acquires certified vendor EPs.

use crate::{
    environment::winml_runtime as runtime_install, execution::operations::PersonOperation,
    inference::face::*,
};
use libloading::Library;
use ort::{environment::Environment, memory::DeviceType};
use std::{
    collections::HashSet,
    ffi::{CStr, c_char, c_void},
    path::Path,
    ptr,
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[cfg(test)]
mod benchmark;

type Handle = *mut c_void;
#[repr(C)]
struct EpInfo {
    name: *const c_char,
    version: *const c_char,
    package_family: *const c_char,
    library: *const c_char,
    package_root: *const c_char,
    ready: i32,
    certification: i32,
}
#[repr(C)]
struct AsyncBlock {
    context: *mut c_void,
    callback: Option<unsafe extern "system" fn(*mut AsyncBlock)>,
    progress: Option<unsafe extern "system" fn(*mut AsyncBlock, f64)>,
}

unsafe extern "system" fn download_progress(block: *mut AsyncBlock, progress: f64) {
    // SAFETY: the operation is borrowed until AsyncClose drains callbacks.
    let operation = unsafe { &*((*block).context.cast::<PersonOperation>()) };
    if progress.is_finite() {
        operation.update(|s| {
            s.detail = format!(
                "正在下载 Windows ML 硬件加速组件：{:.0}%",
                progress.clamp(0.0, 100.0)
            );
        });
    }
}
type Enumerate = unsafe extern "system" fn(
    Handle,
    unsafe extern "system" fn(Handle, *const EpInfo, *mut c_void) -> i32,
    *mut c_void,
) -> i32;

fn check(hr: i32) -> Result<(), String> {
    if hr < 0 {
        Err(format!("Windows ML HRESULT 0x{:08X}", hr as u32))
    } else {
        Ok(())
    }
}

// The catalog owns EP handles until release. Copy names during enumeration;
// no application callbacks or fallible work cross the C callback boundary.
unsafe extern "system" fn collect(ep: Handle, info: *const EpInfo, context: *mut c_void) -> i32 {
    // SAFETY: EnumProviders supplies a valid info and our live Vec context.
    unsafe {
        if !info.is_null() && (*info).certification == 1 && !(*info).name.is_null() {
            let name = CStr::from_ptr((*info).name).to_string_lossy().into_owned();
            (*(context.cast::<Vec<(Handle, String)>>())).push((ep, name));
        }
    }
    1
}

struct Catalog {
    handle: Handle,
    release: unsafe extern "system" fn(Handle),
}
impl Drop for Catalog {
    fn drop(&mut self) {
        // SAFETY: handle was created by this DLL, which outlives the guard.
        unsafe { (self.release)(self.handle) };
    }
}

fn install_and_register(
    runtime: &Path,
    operation: &PersonOperation,
) -> Result<Vec<String>, String> {
    static REGISTERED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    // Catalog initialization installs callbacks used by ORT devices. Keep the
    // verified library loaded for the process, like the ORT environment itself.
    static CATALOG_DLL: OnceLock<Library> = OnceLock::new();
    let mut registered = REGISTERED
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|e| e.to_string())?;
    let env = Environment::current().map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    // SAFETY: runtime_install verified this exact pinned DLL, and these symbols
    // and repr(C) layouts match its shipped headers. It outlives all calls/handles.
    unsafe {
        if CATALOG_DLL.get().is_none() {
            let dll =
                Library::new(runtime.with_file_name("Microsoft.Windows.AI.MachineLearning.dll"))
                    .map_err(|e| e.to_string())?;
            let _ = CATALOG_DLL.set(dll);
        }
        let dll = CATALOG_DLL.get().ok_or("Windows ML DLL 未加载")?;
        let create = dll
            .get::<unsafe extern "system" fn(*mut Handle) -> i32>(b"WinMLEpCatalogCreate\0")
            .map_err(|e| e.to_string())?;
        let release = *dll
            .get::<unsafe extern "system" fn(Handle)>(b"WinMLEpCatalogRelease\0")
            .map_err(|e| e.to_string())?;
        let enumerate = dll
            .get::<Enumerate>(b"WinMLEpCatalogEnumProviders\0")
            .map_err(|e| e.to_string())?;
        let ensure = dll
            .get::<unsafe extern "system" fn(Handle, *mut AsyncBlock) -> i32>(
                b"WinMLEpEnsureReadyAsync\0",
            )
            .map_err(|e| e.to_string())?;
        let status = dll
            .get::<unsafe extern "system" fn(*mut AsyncBlock, i32) -> i32>(b"WinMLAsyncGetStatus\0")
            .map_err(|e| e.to_string())?;
        let cancel = dll
            .get::<unsafe extern "system" fn(*mut AsyncBlock) -> i32>(b"WinMLAsyncCancel\0")
            .map_err(|e| e.to_string())?;
        let close = dll
            .get::<unsafe extern "system" fn(*mut AsyncBlock)>(b"WinMLAsyncClose\0")
            .map_err(|e| e.to_string())?;
        let size = dll
            .get::<unsafe extern "system" fn(Handle, *mut usize) -> i32>(
                b"WinMLEpGetLibraryPathSize\0",
            )
            .map_err(|e| e.to_string())?;
        let path = dll
            .get::<unsafe extern "system" fn(Handle, usize, *mut c_char, *mut usize) -> i32>(
                b"WinMLEpGetLibraryPath\0",
            )
            .map_err(|e| e.to_string())?;
        let mut handle = ptr::null_mut();
        check(create(&mut handle))?;
        let catalog = Catalog { handle, release };
        let mut providers = Vec::<(Handle, String)>::new();
        check(enumerate(
            catalog.handle,
            collect,
            (&mut providers as *mut Vec<_>).cast(),
        ))?;
        for (ep, name) in providers {
            if operation.is_cancelled() {
                return Err("已取消".into());
            }
            if registered.contains(&name) {
                continue;
            }
            operation.update(|s| s.detail = format!("正在准备 {name}（首次使用可能需要下载）…"));
            let result = (|| {
                let mut block = AsyncBlock {
                    context: (operation as *const PersonOperation).cast_mut().cast(),
                    callback: None,
                    progress: Some(download_progress),
                };
                let start = ensure(ep, &mut block);
                if start < 0 {
                    close(&mut block);
                    return check(start);
                }
                let ready = loop {
                    let hr = status(&mut block, 0);
                    // E_PENDING: keep the block alive until Windows completes.
                    if hr != 0x8000000A_u32 as i32 {
                        break hr;
                    }
                    if operation.is_cancelled() {
                        let _ = cancel(&mut block);
                        break status(&mut block, 1);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                };
                close(&mut block);
                if operation.is_cancelled() {
                    return Err("已取消".into());
                }
                check(ready)?;
                let mut length = 0;
                check(size(ep, &mut length))?;
                if length == 0 || length > 32768 {
                    return Err("无效的 EP 路径长度".into());
                }
                let mut buffer = vec![0_u8; length];
                check(path(
                    ep,
                    length,
                    buffer.as_mut_ptr().cast(),
                    ptr::null_mut(),
                ))?;
                let library = CStr::from_bytes_until_nul(&buffer)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .map_err(|e| e.to_string())?;
                if !Path::new(library).is_absolute() {
                    return Err("EP 路径不是绝对路径".into());
                }
                // Registration is environment-owned; this handle only enables
                // explicit unregistration and has no unregister-on-drop behavior.
                let _ = env
                    .register_ep_library(name.clone(), library)
                    .map_err(|e| e.to_string())?;
                registered.insert(name.clone());
                Ok(())
            })();
            if let Err(error) = result {
                warnings.push(format!("{name}: {error}"));
            }
        }
    }
    Ok(warnings)
}

fn device_priority(ep: &str, kind: DeviceType) -> Option<u8> {
    if ep.eq_ignore_ascii_case("NvTensorRTRTXExecutionProvider") && kind == DeviceType::GPU {
        return Some(0);
    }
    match (ep, kind) {
        ("OpenVINOExecutionProvider", DeviceType::GPU) => Some(1),
        (_, DeviceType::GPU) => Some(2),
        ("OpenVINOExecutionProvider", DeviceType::NPU) => Some(3),
        (_, DeviceType::NPU) => Some(4),
        ("OpenVINOExecutionProvider", DeviceType::CPU) => Some(5),
        _ => None,
    }
}

/// Called by the serial background worker, never by folder opening or the UI.
pub fn load_models(root: &Path, operation: &PersonOperation) -> Result<OnnxFaceModels, String> {
    if operation.is_cancelled() {
        return Err("已取消".into());
    }
    let runtime = runtime_install::installed(root)?.ok_or("请先下载运行时")?;
    initialize_runtime(&runtime).map_err(|e| e.to_string())?;
    operation.update(|s| s.detail = "正在检测 Windows ML 硬件加速…".into());
    let mut warnings = match install_and_register(&runtime, operation) {
        Ok(warnings) => warnings,
        Err(error) => vec![format!("EP 目录: {error}")],
    };
    let env = Environment::current().map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    let mut devices: Vec<_> = env.devices().enumerate().collect();
    devices.sort_by_key(|(_, device)| {
        device_priority(device.ep().unwrap_or(""), device.hardware_device().ty()).unwrap_or(u8::MAX)
    });
    for (index, device) in devices {
        let kind = device.hardware_device().ty();
        if device_priority(device.ep().map_err(|e| e.to_string())?, kind).is_some() {
            candidates.push((
                OnnxProvider::WindowsDevice {
                    index,
                    intra_threads: 2,
                },
                format!(
                    "{} / {kind:?} / device {}",
                    device.ep().map_err(|e| e.to_string())?,
                    device.hardware_device().id()
                ),
            ));
        }
    }
    candidates.push((
        OnnxProvider::DirectMl {
            device_id: 0,
            intra_threads: 2,
        },
        "DirectML / GPU 0".into(),
    ));
    candidates.push((OnnxProvider::Cpu { intra_threads: 2 }, "CPU".into()));
    let manifest = crate::environment::catalog::manifest();
    for (provider, label) in candidates {
        if operation.is_cancelled() {
            return Err("已取消".into());
        }
        operation.update(|s| s.detail = format!("正在加载模型：{label}…"));
        let result = OnnxFaceModels::load_verified(OnnxFaceModelRequest {
            manifest: &manifest,
            detector_stage_id: crate::environment::catalog::DETECTOR,
            encoder_stage_id: crate::environment::catalog::ENCODER,
            store_dir: root,
            runtime_library: &runtime,
            runtime_sha256: runtime_install::RUNTIME_SHA,
            directml_sha256: Some(runtime_install::DML_SHA),
            detector_canvas: 960,
            providers: OnnxFaceProviders {
                detector: provider,
                encoder: provider,
            },
        })
        .and_then(|mut models| {
            if operation.is_cancelled() {
                return Err(OnnxFaceError::Ort("已取消".into()));
            }
            operation.update(|s| s.detail = format!("正在验证硬件推理：{label}…"));
            // Validate both actual Run paths before enrolling any photos. This
            // catches unsupported dynamic operators that pass session creation.
            let image = image::RgbImage::from_pixel(112, 112, image::Rgb([127, 127, 127]));
            models.detect_raw(&image)?;
            if operation.is_cancelled() {
                return Err(OnnxFaceError::Ort("已取消".into()));
            }
            models.encode_face(
                &image,
                [
                    [38.0, 52.0],
                    [74.0, 52.0],
                    [56.0, 72.0],
                    [42.0, 92.0],
                    [71.0, 92.0],
                ],
            )?;
            Ok(models)
        });
        match result {
            Ok(models) => {
                if operation.is_cancelled() {
                    return Err("已取消".into());
                }
                operation.update(|s| {
                    s.detail = format!("已启用 {label}");
                    if !warnings.is_empty() {
                        s.error = Some(warnings.join("\n"));
                    }
                });
                return Ok(models);
            }
            Err(error) => warnings.push(format!("{label}: {error}")),
        }
    }
    Err(warnings.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::operations::PersonOperations;
    use oxy_domain::{PersonOperationState, PersonOperationStatus};

    fn operation(operations: &PersonOperations) -> std::sync::Arc<PersonOperation> {
        operations
            .reserve(PersonOperationStatus {
                operation_id: "ep-test".into(),
                folder_path: None,
                state: PersonOperationState::Preparing,
                completed: 0,
                total: 0,
                detail: String::new(),
                error: None,
                run: None,
            })
            .unwrap()
    }

    #[test]
    fn cancelled_preparation_never_loads_or_downloads() {
        let operations = PersonOperations::default();
        let work = operation(&operations);
        operations.cancel("ep-test").unwrap();
        assert_eq!(
            load_models(Path::new("missing-runtime"), &work)
                .err()
                .unwrap(),
            "已取消"
        );
    }

    #[test]
    fn vendor_eps_precede_generic_gpu_and_cpu() {
        let mut devices = [
            ("DmlExecutionProvider", DeviceType::GPU),
            ("OpenVINOExecutionProvider", DeviceType::CPU),
            ("OpenVINOExecutionProvider", DeviceType::GPU),
            ("NvTensorRTRTXExecutionProvider", DeviceType::GPU),
        ];
        devices.sort_by_key(|(name, kind)| device_priority(name, *kind));
        assert_eq!(devices[0].0, "NvTensorRTRTXExecutionProvider");
        assert_eq!(devices[1], ("OpenVINOExecutionProvider", DeviceType::GPU));
        assert_eq!(devices[2].0, "DmlExecutionProvider");
        assert_eq!(
            device_priority("CPUExecutionProvider", DeviceType::CPU),
            None
        );
    }

    #[test]
    fn catalog_callback_only_keeps_certified_providers() {
        let mut entries = Vec::<(Handle, String)>::new();
        let mut info = EpInfo {
            name: c"test".as_ptr(),
            version: ptr::null(),
            package_family: ptr::null(),
            library: ptr::null(),
            package_root: ptr::null(),
            ready: 2,
            certification: 2,
        };
        // SAFETY: all callback arguments refer to live test values.
        unsafe {
            collect(ptr::null_mut(), &info, (&mut entries as *mut Vec<_>).cast());
            assert!(entries.is_empty());
            info.certification = 1;
            collect(ptr::null_mut(), &info, (&mut entries as *mut Vec<_>).cast());
        }
        assert_eq!(entries[0].1, "test");
    }

    #[test]
    #[ignore = "downloads compatible certified EPs; requires installed models and a real JPEG"]
    fn installed_models_use_automatic_ep_and_real_image() {
        let root = std::env::var_os("OXY_TEST_MODEL_ROOT").unwrap();
        let image_path = std::env::var_os("OXY_TEST_FACE_IMAGE").unwrap();
        let operations = PersonOperations::default();
        let work = operation(&operations);
        let started = std::time::Instant::now();
        let mut models = load_models(Path::new(&root), &work).unwrap();
        eprintln!(
            "preparation={:?}; status={:?}",
            started.elapsed(),
            work.snapshot().unwrap()
        );
        let image = image::open(image_path).unwrap().to_rgb8();
        let started = std::time::Instant::now();
        let (geometry, tensors) = models.detect_raw(&image).unwrap();
        assert_eq!(tensors.len(), 9);
        eprintln!(
            "backend={}; detection={:?}; scale={}",
            models.providers().detector.description(),
            started.elapsed(),
            geometry.scale
        );
        let faces = models.detect_faces(&image, 0.5, 0.4).unwrap();
        eprintln!("real_image_faces={}", faces.len());
        assert!(
            !faces.is_empty(),
            "fixture must exercise real face encoding"
        );
        let started = std::time::Instant::now();
        let embedding = models.encode_face(&image, faces[0].landmarks).unwrap();
        assert_eq!(embedding.len(), 512);
        eprintln!("real_face_encoding={:?}", started.elapsed());
        let runtime = runtime_install::installed(Path::new(&root))
            .unwrap()
            .unwrap();
        let manifest = crate::environment::catalog::manifest();
        let cpu = OnnxProvider::Cpu { intra_threads: 2 };
        let mut reference = OnnxFaceModels::load_verified(OnnxFaceModelRequest {
            manifest: &manifest,
            detector_stage_id: crate::environment::catalog::DETECTOR,
            encoder_stage_id: crate::environment::catalog::ENCODER,
            store_dir: Path::new(&root),
            runtime_library: &runtime,
            runtime_sha256: runtime_install::RUNTIME_SHA,
            directml_sha256: Some(runtime_install::DML_SHA),
            detector_canvas: 960,
            providers: OnnxFaceProviders {
                detector: cpu,
                encoder: cpu,
            },
        })
        .unwrap();
        let cpu_faces = reference.detect_faces(&image, 0.5, 0.4).unwrap();
        assert_eq!(faces.len(), cpu_faces.len());
        let cpu_embedding = reference.encode_face(&image, faces[0].landmarks).unwrap();
        let cosine: f32 = embedding
            .iter()
            .zip(cpu_embedding)
            .map(|(a, b)| a * b)
            .sum();
        eprintln!("hardware_cpu_embedding_cosine={cosine}");
        assert!(cosine > 0.99, "hardware output diverges from CPU: {cosine}");
        assert!(
            !matches!(models.providers().detector, OnnxProvider::Cpu { .. }),
            "hardware verification must not silently pass on CPU"
        );
    }
}
