// Copyright (C) 2017 Christopher R. Field.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_use] extern crate log;
extern crate toml;

use std::default::Default;
use std::error::Error as StdError;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use toml::Value;

const CARGO_MANIFEST_FILE: &str = "Cargo.toml";
const WIX_TOOLSET_COMPILER: &str = "candle";
const WIX_TOOLSET_LINKER: &str = "light";
const SIGNTOOL: &str = "signtool";

/// The template, or example, WiX Source (WXS) file.
static TEMPLATE: &str = include_str!("template.wxs");

/// Prints the template to stdout
pub fn print_template() -> Result<(), Error> {
    io::stdout().write(TEMPLATE.as_bytes())?;
    Ok(())
}

#[derive(Debug)]
pub enum Error {
    /// A build operation for the release binary failed.
    Build(String),
    /// A compiler operation failed.
    Compile(String),
    /// A generic or custom error occurred. The message should contain the detailed information.
    Generic(String),
    /// An I/O operation failed.
    Io(io::Error),
    /// A linker operation failed.
    Link(String),
    /// A needed field within the `Cargo.toml` manifest could not be found.
    Manifest(String),
    /// A signing operation failed.
    Sign(String),
    /// Parsing of the `Cargo.toml` manifest failed.
    Toml(toml::de::Error),
}

impl Error {
    /// Gets an error code related to the error.
    ///
    /// This is useful as a return, or exit, code for a command line application, where a non-zero
    /// integer indicates a failure in the application. it can also be used for quickly and easily
    /// testing equality between two errors.
    pub fn code(&self) -> i32 {
        match *self{
            Error::Build(..) => 1,
            Error::Compile(..) => 2,
            Error::Generic(..) => 3,
            Error::Io(..) => 4,
            Error::Link(..) => 5,
            Error::Manifest(..) => 6,
            Error::Sign(..) => 7,
            Error::Toml(..) => 8,
        }
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Build(..) => "Build",
            Error::Compile(..) => "Compile",
            Error::Generic(..) => "Generic",
            Error::Io(..) => "Io",
            Error::Link(..) => "Link",
            Error::Manifest(..) => "Manifest",
            Error::Sign(..) => "Sign",
            Error::Toml(..) => "TOML",
        }
    }

    fn cause(&self) -> Option<&StdError> {
        match *self {
            Error::Io(ref err) => Some(err),
            Error::Toml(ref err) => Some(err),
            _ => None
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::Build(ref msg) => write!(f, "{}", msg),
            Error::Compile(ref msg) => write!(f, "{}", msg),
            Error::Generic(ref msg) => write!(f, "{}", msg),
            Error::Io(ref err) => write!(f, "{}", err),
            Error::Link(ref msg) => write!(f, "{}", msg),
            Error::Manifest(ref var) => write!(f, "No '{}' field found in the package's manifest (Cargo.toml)", var),
            Error::Sign(ref msg) => write!(f, "{}", msg),
            Error::Toml(ref err) => write!(f, "{}", err),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl From<toml::de::Error> for Error {
    fn from(err: toml::de::Error) -> Error {
        Error::Toml(err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Platform {
    X86,
    X64,
}

impl Platform {
    pub fn arch(&self) -> &'static str {
        match *self {
            Platform::X86 => "i686",
            Platform::X64 => "x86_64",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Platform::X86 => write!(f, "x86"),
            Platform::X64 => write!(f, "x64"),
        }
    }
}

impl Default for Platform {
    fn default() -> Self {
        if cfg!(target_arch = "x86_64") {
            Platform::X64
        } else {
            Platform::X86
        }
    }
}

pub struct Wix {
    sign: bool,
    capture_output: bool,
}

impl Wix {
    pub fn new() -> Self {
        Wix {
            sign: false,
            capture_output: true,
        }
    }

    pub fn capture_output(mut self, c: bool) -> Self {
        self.capture_output = c;
        self
    }

    pub fn sign(mut self, s: bool) -> Self {
        self.sign = s;
        self
    }

    /// Runs the subcommand to build the release binary, compile, link, and possibly sign the installer
    /// (msi).
    pub fn run(self) -> Result<(), Error> {
        let cargo_file_path = Path::new(CARGO_MANIFEST_FILE);
        debug!("cargo_file_path = {:?}", cargo_file_path);
        let mut cargo_file = File::open(cargo_file_path)?;
        let mut cargo_file_content = String::new();
        cargo_file.read_to_string(&mut cargo_file_content)?;
        let cargo_values = cargo_file_content.parse::<Value>()?;
        let pkg_version = cargo_values
            .get("package")
            .and_then(|p| p.as_table())
            .and_then(|t| t.get("version"))
            .and_then(|v| v.as_str())
            .ok_or(Error::Manifest(String::from("version")))?;
        debug!("pkg_version = {:?}", pkg_version);
        let pkg_name = cargo_values
            .get("package")
            .and_then(|p| p.as_table())
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .ok_or(Error::Manifest(String::from("name")))?;
        debug!("pkg_name = {:?}", pkg_name);
        let pkg_description = cargo_values
            .get("package")
            .and_then(|p| p.as_table())
            .and_then(|t| t.get("description"))
            .and_then(|d| d.as_str())
            .ok_or(Error::Manifest(String::from("description")))?;
        let pkg_author = cargo_values
            .get("package")
            .and_then(|p| p.as_table())
            .and_then(|t| t.get("authors"))
            .and_then(|a| a.as_array())
            .and_then(|a| a.get(0)) // For now, just use the first author
            .and_then(|f| f.as_str())
            .ok_or(Error::Manifest(String::from("authors")))?;
        debug!("pkg_description = {:?}", pkg_description);
        let bin_name = cargo_values
            .get("bin")
            .and_then(|b| b.as_table())
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(pkg_name);
        debug!("bin_name = {:?}", bin_name);
        let platform = if cfg!(target_arch = "x86_64") {
            Platform::X64
        } else {
            Platform::X86
        };
        debug!("platform = {:?}", platform);
        let mut main_wxs = PathBuf::from("wix");
        main_wxs.push("main");
        main_wxs.set_extension("wxs");
        debug!("main_wxs = {:?}", main_wxs);
        let mut main_wixobj = PathBuf::from("target");
        main_wixobj.push("wix");
        main_wixobj.push("build");
        main_wixobj.push("main");
        main_wixobj.set_extension("wixobj");
        debug!("main_wixobj = {:?}", main_wixobj);
        let mut main_msi = PathBuf::from("target");
        main_msi.push("wix");
        // Do NOT use the `set_extension` method for the MSI path. Since the pkg_version is in X.X.X
        // format, the `set_extension` method will replace the Patch version number and
        // architecture/platform with `msi`.  Instead, just include the extension in the formatted
        // name.
        main_msi.push(&format!("{}-{}-{}.msi", pkg_name, pkg_version, platform.arch()));
        debug!("main_msi = {:?}", main_msi);
        // Build the binary with the release profile. If a release binary has already been built, then
        // this will essentially do nothing.
        info!("Building release binary");
        if let Some(status) = {
            let mut builder = Command::new("cargo");
            if self.capture_output {
                builder.stdout(Stdio::null());
                builder.stderr(Stdio::null());
            }
            builder.arg("build")
                .arg("--release")
                .status()
        }.ok() {
            if !status.success() {
                // TODO: Add better error message
                return Err(Error::Build(String::from("Failed to build the release executable")));
            }
        }
        // Compile the installer
        info!("Compiling installer");
        if let Some(status) = {
            let mut compiler = Command::new(WIX_TOOLSET_COMPILER);
            if self.capture_output {
                compiler.stdout(Stdio::null());
                compiler.stderr(Stdio::null());
            } 
            compiler.arg(format!("-dVersion={}", pkg_version))
                .arg(format!("-dPlatform={}", platform))
                .arg(format!("-dProductName={}", pkg_name))
                .arg(format!("-dBinaryName={}", bin_name))
                .arg(format!("-dDescription={}", pkg_description))
                .arg(format!("-dAuthor={}", pkg_author))
                .arg("-o")
                .arg(&main_wixobj)
                .arg(&main_wxs)
                .status()
        }.ok() {
            if !status.success() {
                // TODO: Add better error message
                return Err(Error::Compile(String::from("Failed to compile the installer")));
            }
        }
        // Link the installer
        info!("Linking the installer");
        if let Some(status) = {
            let mut linker = Command::new(WIX_TOOLSET_LINKER);
            if self.capture_output {    
                linker.stdout(Stdio::null());
                linker.stderr(Stdio::null());
            }
            linker.arg("-ext")
                .arg("WixUIExtension")
                .arg("-cultures:en-us")
                .arg(&main_wixobj)
                .arg("-out")
                .arg(&main_msi)
                .status()
        }.ok() {
            if !status.success() {
                // TODO: Add better error message
                return Err(Error::Link(String::from("Failed to link the installer")));
            }
        }
        // Sign the installer
        if self.sign {
            info!("Signing the installer");
            if let Some(status) = {
                let mut signer = Command::new(SIGNTOOL);
                if self.capture_output {
                    signer.stdout(Stdio::null());
                    signer.stderr(Stdio::null());
                }
                signer.arg("sign")
                    .arg("/a")
                    //.arg("/t")
                    //.arg("http://timestamp.comodoca.com")
                    .arg(&main_msi)
                    .status()
            }.ok() {
                if !status.success() {
                    // TODO: Add better error message
                    return Err(Error::Sign(String::from("Failed to sign the installer")));
                }
            }
        }
        Ok(())
    }
}

impl Default for Wix {
    fn default() -> Self {
        Wix::new()
    }
}
