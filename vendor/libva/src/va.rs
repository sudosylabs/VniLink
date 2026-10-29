// Copyright 2022 The ChromiumOS Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! VA-API dynamic loading abstraction.
//! When the `dlopen` feature is enabled, symbols are loaded at runtime via `libloading`.
//! Otherwise, functions are directly linked and re-exported from `crate::bindings`.

#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::missing_safety_doc)]

#[cfg(not(feature = "dlopen"))]
pub use crate::bindings::{
    vaGetDisplayDRM,
    vaInitialize,
    vaTerminate,
    vaQueryVendorString,
    vaMaxNumProfiles,
    vaQueryConfigProfiles,
    vaMaxNumEntrypoints,
    vaQueryConfigEntrypoints,
    vaGetConfigAttributes,
    vaCreateConfig,
    vaDestroyConfig,
    vaQuerySurfaceAttributes,
    vaCreateSurfaces,
    vaDestroySurfaces,
    vaCreateContext,
    vaDestroyContext,
    vaCreateBuffer,
    vaDestroyBuffer,
    vaMapBuffer,
    vaUnmapBuffer,
    vaCreateImage,
    vaDestroyImage,
    vaDeriveImage,
    vaGetImage,
    vaPutImage,
    vaExportSurfaceHandle,
    vaBeginPicture,
    vaRenderPicture,
    vaEndPicture,
    vaSyncSurface,
    vaQuerySurfaceStatus,
    vaQuerySurfaceError,
    vaMaxNumImageFormats,
    vaQueryImageFormats,
    vaErrorStr,
};

#[cfg(feature = "dlopen")]
mod dynamic {
    use crate::bindings::*;
    use libloading::{Library, Symbol};
    use std::sync::OnceLock;

    static VA_LIB: OnceLock<Result<Library, String>> = OnceLock::new();
    static VA_DRM_LIB: OnceLock<Result<Library, String>> = OnceLock::new();
    static FALLBACK_ERROR_STR: &[u8] = b"libva not loaded\0";

    fn va_lib() -> Result<&'static Library, VAStatus> {
        VA_LIB
            .get_or_init(|| unsafe {
                Library::new("libva.so.2")
                    .or_else(|_| Library::new("libva.so"))
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|_| 1 as VAStatus)
    }

    fn drm_lib() -> Result<&'static Library, VAStatus> {
        VA_DRM_LIB
            .get_or_init(|| unsafe {
                Library::new("libva-drm.so.2")
                    .or_else(|_| Library::new("libva-drm.so"))
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|_| 1 as VAStatus)
    }

    fn any_lib(symbol: &[u8]) -> Option<&'static Library> {
        if let Ok(lib) = va_lib() {
            unsafe {
                if lib.get::<*mut std::os::raw::c_void>(symbol).is_ok() {
                    return Some(lib);
                }
            }
        }
        if let Ok(lib) = drm_lib() {
            unsafe {
                if lib.get::<*mut std::os::raw::c_void>(symbol).is_ok() {
                    return Some(lib);
                }
            }
        }
        None
    }

    pub unsafe fn vaGetDisplayDRM(fd: ::std::os::raw::c_int) -> VADisplay {
        let lib = match drm_lib().or_else(|_| va_lib()) {
            Ok(l) => l,
            Err(_) => return std::ptr::null_mut(),
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(fd: ::std::os::raw::c_int) -> VADisplay> = match lib.get(concat!(stringify!(vaGetDisplayDRM), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return std::ptr::null_mut(),
            };
            func(fd)
        }
    }

    pub unsafe fn vaInitialize(dpy: VADisplay, major_version: *mut ::std::os::raw::c_int, minor_version: *mut ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaInitialize), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, major_version: *mut ::std::os::raw::c_int, minor_version: *mut ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaInitialize), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, major_version, minor_version)
        }
    }

    pub unsafe fn vaTerminate(dpy: VADisplay) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaTerminate), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay) -> VAStatus> = match lib.get(concat!(stringify!(vaTerminate), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy)
        }
    }

    pub unsafe fn vaQueryVendorString(dpy: VADisplay) -> *const ::std::os::raw::c_char {
        let lib = match any_lib(concat!(stringify!(vaQueryVendorString), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return std::ptr::null(),
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay) -> *const ::std::os::raw::c_char> = match lib.get(concat!(stringify!(vaQueryVendorString), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return std::ptr::null(),
            };
            func(dpy)
        }
    }

    pub unsafe fn vaMaxNumProfiles(dpy: VADisplay) -> ::std::os::raw::c_int {
        let lib = match any_lib(concat!(stringify!(vaMaxNumProfiles), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 0,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay) -> ::std::os::raw::c_int> = match lib.get(concat!(stringify!(vaMaxNumProfiles), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 0,
            };
            func(dpy)
        }
    }

    pub unsafe fn vaQueryConfigProfiles(dpy: VADisplay, profile_list: *mut VAProfile::Type, num_profiles: *mut ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQueryConfigProfiles), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, profile_list: *mut VAProfile::Type, num_profiles: *mut ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaQueryConfigProfiles), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, profile_list, num_profiles)
        }
    }

    pub unsafe fn vaMaxNumEntrypoints(dpy: VADisplay) -> ::std::os::raw::c_int {
        let lib = match any_lib(concat!(stringify!(vaMaxNumEntrypoints), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 0,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay) -> ::std::os::raw::c_int> = match lib.get(concat!(stringify!(vaMaxNumEntrypoints), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 0,
            };
            func(dpy)
        }
    }

    pub unsafe fn vaQueryConfigEntrypoints(dpy: VADisplay, profile: VAProfile::Type, entrypoint_list: *mut VAEntrypoint::Type, num_entrypoints: *mut ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQueryConfigEntrypoints), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, profile: VAProfile::Type, entrypoint_list: *mut VAEntrypoint::Type, num_entrypoints: *mut ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaQueryConfigEntrypoints), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, profile, entrypoint_list, num_entrypoints)
        }
    }

    pub unsafe fn vaGetConfigAttributes(dpy: VADisplay, profile: VAProfile::Type, entrypoint: VAEntrypoint::Type, attrib_list: *mut VAConfigAttrib, num_attribs: ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaGetConfigAttributes), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, profile: VAProfile::Type, entrypoint: VAEntrypoint::Type, attrib_list: *mut VAConfigAttrib, num_attribs: ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaGetConfigAttributes), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, profile, entrypoint, attrib_list, num_attribs)
        }
    }

    pub unsafe fn vaCreateConfig(dpy: VADisplay, profile: VAProfile::Type, entrypoint: VAEntrypoint::Type, attrib_list: *mut VAConfigAttrib, num_attribs: ::std::os::raw::c_int, config_id: *mut VAConfigID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaCreateConfig), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, profile: VAProfile::Type, entrypoint: VAEntrypoint::Type, attrib_list: *mut VAConfigAttrib, num_attribs: ::std::os::raw::c_int, config_id: *mut VAConfigID) -> VAStatus> = match lib.get(concat!(stringify!(vaCreateConfig), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, profile, entrypoint, attrib_list, num_attribs, config_id)
        }
    }

    pub unsafe fn vaDestroyConfig(dpy: VADisplay, config_id: VAConfigID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDestroyConfig), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, config_id: VAConfigID) -> VAStatus> = match lib.get(concat!(stringify!(vaDestroyConfig), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, config_id)
        }
    }

    pub unsafe fn vaQuerySurfaceAttributes(dpy: VADisplay, config: VAConfigID, attrib_list: *mut VASurfaceAttrib, num_attribs: *mut ::std::os::raw::c_uint) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQuerySurfaceAttributes), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, config: VAConfigID, attrib_list: *mut VASurfaceAttrib, num_attribs: *mut ::std::os::raw::c_uint) -> VAStatus> = match lib.get(concat!(stringify!(vaQuerySurfaceAttributes), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, config, attrib_list, num_attribs)
        }
    }

    pub unsafe fn vaCreateSurfaces(dpy: VADisplay, format: ::std::os::raw::c_uint, width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint, surfaces: *mut VASurfaceID, num_surfaces: ::std::os::raw::c_uint, attrib_list: *mut VASurfaceAttrib, num_attribs: ::std::os::raw::c_uint) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaCreateSurfaces), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, format: ::std::os::raw::c_uint, width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint, surfaces: *mut VASurfaceID, num_surfaces: ::std::os::raw::c_uint, attrib_list: *mut VASurfaceAttrib, num_attribs: ::std::os::raw::c_uint) -> VAStatus> = match lib.get(concat!(stringify!(vaCreateSurfaces), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, format, width, height, surfaces, num_surfaces, attrib_list, num_attribs)
        }
    }

    pub unsafe fn vaDestroySurfaces(dpy: VADisplay, surfaces: *mut VASurfaceID, num_surfaces: ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDestroySurfaces), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surfaces: *mut VASurfaceID, num_surfaces: ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaDestroySurfaces), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surfaces, num_surfaces)
        }
    }

    pub unsafe fn vaCreateContext(dpy: VADisplay, config_id: VAConfigID, picture_width: ::std::os::raw::c_int, picture_height: ::std::os::raw::c_int, flag: ::std::os::raw::c_int, render_targets: *mut VASurfaceID, num_render_targets: ::std::os::raw::c_int, context: *mut VAContextID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaCreateContext), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, config_id: VAConfigID, picture_width: ::std::os::raw::c_int, picture_height: ::std::os::raw::c_int, flag: ::std::os::raw::c_int, render_targets: *mut VASurfaceID, num_render_targets: ::std::os::raw::c_int, context: *mut VAContextID) -> VAStatus> = match lib.get(concat!(stringify!(vaCreateContext), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, config_id, picture_width, picture_height, flag, render_targets, num_render_targets, context)
        }
    }

    pub unsafe fn vaDestroyContext(dpy: VADisplay, context: VAContextID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDestroyContext), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, context: VAContextID) -> VAStatus> = match lib.get(concat!(stringify!(vaDestroyContext), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, context)
        }
    }

    pub unsafe fn vaCreateBuffer(dpy: VADisplay, context: VAContextID, type_: VABufferType::Type, size: ::std::os::raw::c_uint, num_elements: ::std::os::raw::c_uint, data: *mut ::std::os::raw::c_void, buf_id: *mut VABufferID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaCreateBuffer), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, context: VAContextID, type_: VABufferType::Type, size: ::std::os::raw::c_uint, num_elements: ::std::os::raw::c_uint, data: *mut ::std::os::raw::c_void, buf_id: *mut VABufferID) -> VAStatus> = match lib.get(concat!(stringify!(vaCreateBuffer), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, context, type_, size, num_elements, data, buf_id)
        }
    }

    pub unsafe fn vaDestroyBuffer(dpy: VADisplay, buffer_id: VABufferID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDestroyBuffer), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, buffer_id: VABufferID) -> VAStatus> = match lib.get(concat!(stringify!(vaDestroyBuffer), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, buffer_id)
        }
    }

    pub unsafe fn vaMapBuffer(dpy: VADisplay, buf_id: VABufferID, pbuf: *mut *mut ::std::os::raw::c_void) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaMapBuffer), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, buf_id: VABufferID, pbuf: *mut *mut ::std::os::raw::c_void) -> VAStatus> = match lib.get(concat!(stringify!(vaMapBuffer), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, buf_id, pbuf)
        }
    }

    pub unsafe fn vaUnmapBuffer(dpy: VADisplay, buf_id: VABufferID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaUnmapBuffer), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, buf_id: VABufferID) -> VAStatus> = match lib.get(concat!(stringify!(vaUnmapBuffer), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, buf_id)
        }
    }

    pub unsafe fn vaCreateImage(dpy: VADisplay, format: *mut VAImageFormat, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int, image: *mut VAImage) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaCreateImage), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, format: *mut VAImageFormat, width: ::std::os::raw::c_int, height: ::std::os::raw::c_int, image: *mut VAImage) -> VAStatus> = match lib.get(concat!(stringify!(vaCreateImage), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, format, width, height, image)
        }
    }

    pub unsafe fn vaDestroyImage(dpy: VADisplay, image: VAImageID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDestroyImage), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, image: VAImageID) -> VAStatus> = match lib.get(concat!(stringify!(vaDestroyImage), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, image)
        }
    }

    pub unsafe fn vaDeriveImage(dpy: VADisplay, surface: VASurfaceID, image: *mut VAImage) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaDeriveImage), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surface: VASurfaceID, image: *mut VAImage) -> VAStatus> = match lib.get(concat!(stringify!(vaDeriveImage), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surface, image)
        }
    }

    pub unsafe fn vaGetImage(dpy: VADisplay, surface: VASurfaceID, x: ::std::os::raw::c_int, y: ::std::os::raw::c_int, width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint, image: VAImageID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaGetImage), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surface: VASurfaceID, x: ::std::os::raw::c_int, y: ::std::os::raw::c_int, width: ::std::os::raw::c_uint, height: ::std::os::raw::c_uint, image: VAImageID) -> VAStatus> = match lib.get(concat!(stringify!(vaGetImage), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surface, x, y, width, height, image)
        }
    }

    pub unsafe fn vaPutImage(dpy: VADisplay, surface: VASurfaceID, image: VAImageID, src_x: ::std::os::raw::c_int, src_y: ::std::os::raw::c_int, src_width: ::std::os::raw::c_uint, src_height: ::std::os::raw::c_uint, dest_x: ::std::os::raw::c_int, dest_y: ::std::os::raw::c_int, dest_width: ::std::os::raw::c_uint, dest_height: ::std::os::raw::c_uint) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaPutImage), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surface: VASurfaceID, image: VAImageID, src_x: ::std::os::raw::c_int, src_y: ::std::os::raw::c_int, src_width: ::std::os::raw::c_uint, src_height: ::std::os::raw::c_uint, dest_x: ::std::os::raw::c_int, dest_y: ::std::os::raw::c_int, dest_width: ::std::os::raw::c_uint, dest_height: ::std::os::raw::c_uint) -> VAStatus> = match lib.get(concat!(stringify!(vaPutImage), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surface, image, src_x, src_y, src_width, src_height, dest_x, dest_y, dest_width, dest_height)
        }
    }

    pub unsafe fn vaExportSurfaceHandle(dpy: VADisplay, surface_id: VASurfaceID, mem_type: u32, flags: u32, descriptor: *mut ::std::os::raw::c_void) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaExportSurfaceHandle), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surface_id: VASurfaceID, mem_type: u32, flags: u32, descriptor: *mut ::std::os::raw::c_void) -> VAStatus> = match lib.get(concat!(stringify!(vaExportSurfaceHandle), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surface_id, mem_type, flags, descriptor)
        }
    }

    pub unsafe fn vaBeginPicture(dpy: VADisplay, context: VAContextID, render_target: VASurfaceID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaBeginPicture), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, context: VAContextID, render_target: VASurfaceID) -> VAStatus> = match lib.get(concat!(stringify!(vaBeginPicture), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, context, render_target)
        }
    }

    pub unsafe fn vaRenderPicture(dpy: VADisplay, context: VAContextID, buffers: *mut VABufferID, num_buffers: ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaRenderPicture), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, context: VAContextID, buffers: *mut VABufferID, num_buffers: ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaRenderPicture), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, context, buffers, num_buffers)
        }
    }

    pub unsafe fn vaEndPicture(dpy: VADisplay, context: VAContextID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaEndPicture), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, context: VAContextID) -> VAStatus> = match lib.get(concat!(stringify!(vaEndPicture), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, context)
        }
    }

    pub unsafe fn vaSyncSurface(dpy: VADisplay, render_target: VASurfaceID) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaSyncSurface), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, render_target: VASurfaceID) -> VAStatus> = match lib.get(concat!(stringify!(vaSyncSurface), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, render_target)
        }
    }

    pub unsafe fn vaQuerySurfaceStatus(dpy: VADisplay, render_target: VASurfaceID, status: *mut VASurfaceStatus::Type) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQuerySurfaceStatus), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, render_target: VASurfaceID, status: *mut VASurfaceStatus::Type) -> VAStatus> = match lib.get(concat!(stringify!(vaQuerySurfaceStatus), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, render_target, status)
        }
    }

    pub unsafe fn vaQuerySurfaceError(dpy: VADisplay, surface: VASurfaceID, error_status: VAStatus, error_info: *mut *mut ::std::os::raw::c_void) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQuerySurfaceError), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, surface: VASurfaceID, error_status: VAStatus, error_info: *mut *mut ::std::os::raw::c_void) -> VAStatus> = match lib.get(concat!(stringify!(vaQuerySurfaceError), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, surface, error_status, error_info)
        }
    }

    pub unsafe fn vaMaxNumImageFormats(dpy: VADisplay) -> ::std::os::raw::c_int {
        let lib = match any_lib(concat!(stringify!(vaMaxNumImageFormats), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 0,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay) -> ::std::os::raw::c_int> = match lib.get(concat!(stringify!(vaMaxNumImageFormats), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 0,
            };
            func(dpy)
        }
    }

    pub unsafe fn vaQueryImageFormats(dpy: VADisplay, format_list: *mut VAImageFormat, num_formats: *mut ::std::os::raw::c_int) -> VAStatus {
        let lib = match any_lib(concat!(stringify!(vaQueryImageFormats), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return 1,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(dpy: VADisplay, format_list: *mut VAImageFormat, num_formats: *mut ::std::os::raw::c_int) -> VAStatus> = match lib.get(concat!(stringify!(vaQueryImageFormats), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return 1,
            };
            func(dpy, format_list, num_formats)
        }
    }

    pub unsafe fn vaErrorStr(error_status: VAStatus) -> *const ::std::os::raw::c_char {
        let lib = match any_lib(concat!(stringify!(vaErrorStr), "\0").as_bytes()).or_else(|| va_lib().ok()).or_else(|| drm_lib().ok()) {
            Some(l) => l,
            None => return FALLBACK_ERROR_STR.as_ptr() as *const ::std::os::raw::c_char,
        };
        unsafe {
            let func: Symbol<unsafe extern "C" fn(error_status: VAStatus) -> *const ::std::os::raw::c_char> = match lib.get(concat!(stringify!(vaErrorStr), "\0").as_bytes()) {
                Ok(f) => f,
                Err(_) => return FALLBACK_ERROR_STR.as_ptr() as *const ::std::os::raw::c_char,
            };
            func(error_status)
        }
    }

}

#[cfg(feature = "dlopen")]
pub use dynamic::{
    vaGetDisplayDRM,
    vaInitialize,
    vaTerminate,
    vaQueryVendorString,
    vaMaxNumProfiles,
    vaQueryConfigProfiles,
    vaMaxNumEntrypoints,
    vaQueryConfigEntrypoints,
    vaGetConfigAttributes,
    vaCreateConfig,
    vaDestroyConfig,
    vaQuerySurfaceAttributes,
    vaCreateSurfaces,
    vaDestroySurfaces,
    vaCreateContext,
    vaDestroyContext,
    vaCreateBuffer,
    vaDestroyBuffer,
    vaMapBuffer,
    vaUnmapBuffer,
    vaCreateImage,
    vaDestroyImage,
    vaDeriveImage,
    vaGetImage,
    vaPutImage,
    vaExportSurfaceHandle,
    vaBeginPicture,
    vaRenderPicture,
    vaEndPicture,
    vaSyncSurface,
    vaQuerySurfaceStatus,
    vaQuerySurfaceError,
    vaMaxNumImageFormats,
    vaQueryImageFormats,
    vaErrorStr,
};

