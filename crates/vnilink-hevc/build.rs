// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::{env, fs, path::Path};
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        } else {
            let destination = to.join(entry.file_name());
            // Preserve timestamps so a shim-only change does not rebuild both codec libraries.
            let source = fs::read(entry.path()).unwrap();
            if fs::read(&destination).ok().as_deref() != Some(source.as_slice()) {
                fs::write(destination, source).unwrap();
            }
        }
    }
}
fn main() {
    shiguredo_cmake::set_cmake_env();
    let target = env::var("TARGET").unwrap();
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-changed=../../vendor/hevc");
    let out = std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut installs = Vec::new();
    for (name, options) in [
        (
            "kvazaar-2.3.2",
            vec![
                "BUILD_TESTS",
                "GIT_SUBMODULE",
                "BUILD_KVAZAAR_BINARY",
                "USE_CRYPTO",
            ],
        ),
        (
            "libde265-1.1.3",
            vec![
                "ENABLE_DECODER",
                "ENABLE_ENCODER",
                "ENABLE_SHERLOCK265",
                "ENABLE_INTERNAL_DEVELOPMENT_TOOLS",
                "ENABLE_SDL",
                "WITH_FUZZERS",
            ],
        ),
    ] {
        let source = out.join(name);
        // Upstream install writes generated files into the source directory.
        copy_tree(&Path::new("../../vendor/hevc").join(name), &source);
        let mut config = shiguredo_cmake::Config::new(&source);
        config
            .out_dir(out.join(format!("build-{name}")))
            .profile("Release")
            .define("BUILD_SHARED_LIBS", "OFF")
            .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
            .define("CMAKE_INSTALL_LIBDIR", "lib");
        if target.contains("windows-msvc") {
            config.define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded");
        }
        for option in options {
            config.define(option, "OFF");
        }
        let install = config.build();
        println!(
            "cargo:rustc-link-search=native={}",
            install.join("lib").display()
        );
        installs.push(install);
    }
    let mut compiler = cc::Build::new();
    compiler
        .cpp(true)
        .define("LIBDE265_STATIC_BUILD", None)
        .define("KVZ_STATIC_LIB", None)
        .file("src/shim.cpp")
        .flag_if_supported("-std=c++17");
    if target.contains("windows-msvc") {
        compiler.static_crt(true).debug(false);
    }
    if target.contains("windows-gnu") {
        compiler.cpp_link_stdlib_static(true);
        // rustc bundles static archives itself, so GCC's implicit search paths
        // must be explicit for the C++ and thread runtimes.
        for library in ["libstdc++.a", "libmingwthrd.a"] {
            let output = compiler
                .get_compiler()
                .to_command()
                .arg(format!("-print-file-name={library}"))
                .output()
                .expect("C++ runtime search");
            assert!(output.status.success(), "C++ runtime search failed");
            let archive =
                std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
            assert!(archive.is_file(), "C++ runtime archive is missing");
            println!(
                "cargo:rustc-link-search=native={}",
                archive.parent().unwrap().display()
            );
        }
    }
    for install in &installs {
        compiler.include(install.join("include"));
    }
    compiler.compile("vnilink_hevc_shim");
    println!("cargo:rustc-link-lib=static=kvazaar");
    println!("cargo:rustc-link-lib=static=de265");
    if target.contains("windows-gnu") {
        println!("cargo:rustc-link-lib=static:+whole-archive=mingwthrd");
    }
    if !target.contains("windows") {
        println!("cargo:rustc-link-lib=pthread");
    }
    if target.contains("linux") {
        println!("cargo:rustc-link-lib=m");
    }
}
