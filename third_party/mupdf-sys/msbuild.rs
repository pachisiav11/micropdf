use std::{env, fs, path::Path};

use cc::windows_registry::{self, find_vs_version, VsVers};

use crate::{Result, Target};

#[derive(Default)]
pub struct Msbuild {
    cl: Vec<String>,
}

impl Msbuild {
    pub fn define(&mut self, var: &str, val: &str) {
        self.cl.push(format!("/D{var}#{val}"));
    }

    fn patch_nan(&self, build_dir: &str) -> Result<()> {
        let file_path = Path::new(build_dir).join("source/fitz/geometry.c");
        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read geometry.c: {e}"))?;

        // work around https://developercommunity.visualstudio.com/t/NAN-is-no-longer-compile-time-constant-i/10688907
        let patched_content = content.replace("NAN", "(0.0/0.0)");

        fs::write(&file_path, patched_content)
            .map_err(|e| format!("Failed to write patched geometry.c: {e}"))?;

        Ok(())
    }

    fn remove_libresources_fonts(&self, build_dir: &str) -> Result<()> {
        let file_path = Path::new(build_dir).join("platform/win32/libresources.vcxproj");
        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read libresources.vcxproj: {e}"))?;

        let patched: String = content
            .lines()
            .filter(|line| {
                !line.contains(r"fonts\han\")
                    && !line.contains(r"fonts\droid\")
                    && !line.contains(r"fonts\noto\")
                    && !line.contains(r"fonts\sil\")
            })
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&file_path, patched)
            .map_err(|e| format!("Failed to write patched libresources.vcxproj: {e}"))?;

        Ok(())
    }

    /// micropdf: the solution always builds Tesseract, Leptonica and zxing-cpp into libmupdf and
    /// turns OCR and barcodes on. Leave them out unless their features ask for them.
    fn drop_optional_libraries(&mut self, build_dir: &str) -> Result<()> {
        let file_path = Path::new(build_dir).join("platform/win32/libmupdf.vcxproj");
        let mut content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read libmupdf.vcxproj: {e}"))?;

        if !cfg!(feature = "tesseract") {
            content = content
                .replace("HAVE_TESSERACT;", "")
                .replace("HAVE_LEPTONICA;", "");
            content = drop_reference(&content, "libtesseract.vcxproj")?;
            self.define("FZ_ENABLE_OCR_OUTPUT", "0");
        }
        if !cfg!(feature = "zxingcpp") {
            content = drop_reference(&content, "libmubarcode.vcxproj")?;
            self.define("FZ_ENABLE_BARCODE", "0");
        }

        fs::write(&file_path, content)
            .map_err(|e| format!("Failed to write patched libmupdf.vcxproj: {e}"))?;

        Ok(())
    }

    pub fn build(mut self, target: &Target, build_dir: &str) -> Result<()> {
        self.cl.push("/MP".to_owned());

        self.patch_nan(build_dir)?;
        self.remove_libresources_fonts(build_dir)?;
        self.drop_optional_libraries(build_dir)?;

        // micropdf: always the Release configuration. The Debug one links the debug C runtime,
        // which clashes with the release runtime Rust links (LNK4098). Debug builds skip
        // whole-program optimization so each test binary links quickly.
        let configuration = "Release";
        let whole_program = !target.debug_profile();

        let platform = match &*target.arch {
            "i386" | "i586" | "i686" => "Win32",
            "x86_64" => "x64",
            _ => Err(format!(
                "mupdf currently only supports Win32 and x64 with msvc\n\
                Try compiling using mingw for potential {:?} support",
                target.arch,
            ))?,
        };

        let platform_toolset = env::var("MUPDF_MSVC_PLATFORM_TOOLSET").unwrap_or_else(|_| {
            match find_vs_version() {
                Ok(VsVers::Vs17) => "v143",
                _ => "v142",
            }
            .to_owned()
        });

        let Some(mut msbuild) = windows_registry::find(&target.arch, "msbuild.exe") else {
            Err("Could not find msbuild.exe. Do you have it installed?")?
        };
        let status = msbuild
            .args([
                r"platform\win32\mupdf.sln",
                "/target:libmupdf",
                &format!("/p:Configuration={configuration}"),
                &format!("/p:Platform={platform}"),
                &format!("/p:PlatformToolset={platform_toolset}"),
                &format!("/p:WholeProgramOptimization={whole_program}"),
            ])
            .current_dir(build_dir)
            .env("CL", self.cl.join(" "))
            .status()
            .map_err(|e| format!("Failed to call msbuild: {e}"))?;
        if !status.success() {
            Err(match status.code() {
                Some(code) => format!("msbuild invocation failed with status {code}"),
                None => "msbuild invocation failed".to_owned(),
            })?;
        }

        if platform == "x64" {
            println!(
                "cargo:rustc-link-search=native={build_dir}/platform/win32/x64/{configuration}"
            );
        } else {
            println!("cargo:rustc-link-search=native={build_dir}/platform/win32/{configuration}");
        }

        println!("cargo:rustc-link-lib=dylib=libmupdf");
        println!("cargo:rustc-link-lib=dylib=libthirdparty");

        Ok(())
    }
}

/// Removes the `<ProjectReference>` element for `project`.
fn drop_reference(content: &str, project: &str) -> Result<String> {
    let open = format!("<ProjectReference Include=\"{project}\">");
    let start = content
        .find(&open)
        .ok_or_else(|| format!("libmupdf.vcxproj has no reference to {project}"))?;
    let close = "</ProjectReference>";
    let end = start
        + content[start..]
            .find(close)
            .ok_or("unclosed ProjectReference")?
        + close.len();
    let start = content[..start].rfind('\n').map_or(start, |i| i + 1);
    let end = content[end..].find('\n').map_or(end, |i| end + i + 1);
    Ok(format!("{}{}", &content[..start], &content[end..]))
}
