use anyhow::{format_err, Context, Result};
use std::io::Read;
use strum::IntoEnumIterator;

mod npm;

#[derive(Clone, Debug)]
pub struct JsExtension {
    name_: String,
    registry_host_names_: Vec<String>,
    root_url_: url::Url,
    registry_human_url_template_: String,
}

impl vouch_lib::extension::FromLib for JsExtension {
    fn new() -> Self {
        Self {
            name_: "js".to_string(),
            registry_host_names_: vec!["npmjs.com".to_owned()],
            root_url_: url::Url::parse("https://www.npmjs.com").unwrap(),
            registry_human_url_template_:
                "https://www.npmjs.com/package/{{package_name}}/v/{{package_version}}".to_string(),
        }
    }
}

impl vouch_lib::extension::Extension for JsExtension {
    fn name(&self) -> String {
        self.name_.clone()
    }

    fn registries(&self) -> Vec<String> {
        self.registry_host_names_.clone()
    }

    /// Returns a list of dependencies for the given package.
    ///
    /// Returns one package dependencies structure per registry.
    fn identify_package_dependencies(
        &self,
        package_name: &str,
        package_version: &Option<&str>,
        _extension_args: &Vec<String>,
    ) -> Result<Vec<vouch_lib::extension::PackageDependencies>> {
        // npm install is-even@1.0.0 --package-lock-only
        let tmp_dir = tempdir::TempDir::new("vouch_js_identify_package_dependencies")?;
        let tmp_directory_path = tmp_dir.path().to_path_buf();

        let package = if let Some(package_version) = package_version {
            format!(
                "{name}@{version}",
                name = package_name,
                version = package_version
            )
        } else {
            package_name.to_string()
        };
        let args = vec!["install", package.as_str(), "--package-lock-only"];

        std::process::Command::new("npm")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .current_dir(&tmp_directory_path)
            .output()?;

        let package_lock_path = tmp_directory_path.join("package-lock.json");
        let dependencies = npm::get_dependencies(&package_lock_path, false)?;

        let package_version = if let Some(package_version) = package_version {
            vouch_lib::extension::VersionParseResult::Ok(package_version.to_string())
        } else {
            // Extract target package version from dependencies so as to remove from the dependencies vector.
            let mut target_package_instances: Vec<_> = dependencies
                .iter()
                .filter(|d| d.name == package_name)
                .cloned()
                .collect();
            target_package_instances.sort();
            target_package_instances.reverse();
            let target_package_instance = target_package_instances.first().ok_or(format_err!(
                "Failed to find target package in dependencies list."
            ))?;
            target_package_instance.version.clone()
        };

        let dependencies = dependencies
            .into_iter()
            .filter(|d| d.name != package_name && d.version != package_version)
            .collect();

        Ok(vec![vouch_lib::extension::PackageDependencies {
            package_version: package_version,
            registry_host_name: npm::get_registry_host_name(),
            dependencies: dependencies,
        }])
    }

    fn identify_file_defined_dependencies(
        &self,
        working_directory: &std::path::PathBuf,
        extension_args: &Vec<String>,
    ) -> Result<Vec<vouch_lib::extension::FileDefinedDependencies>> {
        let include_dev_dependencies = extension_args.iter().any(|v| v == "--dev");

        // Identify all dependency definition files.
        let dependency_files = match identify_dependency_files(&working_directory) {
            Some(v) => v,
            None => return Ok(Vec::new()),
        };

        // Read all dependencies definitions files.
        let mut all_dependency_specs = Vec::new();
        for dependency_file in dependency_files {
            // TODO: Add support for parsing all definition file types.
            let (dependencies, registry_host_name) = match dependency_file.r#type {
                DependencyFileType::Npm => (
                    npm::get_dependencies(&dependency_file.path, include_dev_dependencies)?,
                    npm::get_registry_host_name(),
                ),
            };
            all_dependency_specs.push(vouch_lib::extension::FileDefinedDependencies {
                path: dependency_file.path,
                registry_host_name: registry_host_name,
                dependencies: dependencies,
            });
        }

        Ok(all_dependency_specs)
    }

    fn registries_package_metadata(
        &self,
        package_name: &str,
        package_version: &Option<&str>,
    ) -> Result<Vec<vouch_lib::extension::RegistryPackageMetadata>> {
        let package_version = match package_version {
            Some(v) => Some(v.to_string()),
            None => get_latest_version(&package_name)?,
        }
        .ok_or(format_err!("Failed to find package version."))?;

        // Query remote package registry for given package.
        let human_url = get_registry_human_url(&self, &package_name, &package_version)?;

        // Currently, only one registry is supported. Therefore simply extract.
        let registry_host_name = self
            .registries()
            .first()
            .ok_or(format_err!(
                "Code error: vector of registry host names is empty."
            ))?
            .clone();

        let entry_json = get_registry_entry_json(&package_name)?;
        let artifact_url = get_archive_url(&entry_json, &package_version)?;

        Ok(vec![vouch_lib::extension::RegistryPackageMetadata {
            registry_host_name: registry_host_name,
            human_url: human_url.to_string(),
            artifact_url: artifact_url.to_string(),
            is_primary: true,
            package_version: package_version,
        }])
    }
}

/// Given package name, return latest version.
fn get_latest_version(package_name: &str) -> Result<Option<String>> {
    let json = get_registry_entry_json(&package_name)?;
    let versions = json["versions"]
        .as_object()
        .ok_or(format_err!("Failed to find versions JSON section."))?;
    let latest_version = versions.keys().last();
    Ok(latest_version.cloned())
}

fn get_registry_human_url(
    extension: &JsExtension,
    package_name: &str,
    package_version: &str,
) -> Result<url::Url> {
    // Example return value: https://www.npmjs.com/package/d3/v/6.5.0
    let handlebars_registry = handlebars::Handlebars::new();
    let url = handlebars_registry.render_template(
        &extension.registry_human_url_template_,
        &maplit::btreemap! {
            "package_name" => package_name,
            "package_version" => package_version,
        },
    )?;
    Ok(url::Url::parse(url.as_str())?)
}

fn get_registry_entry_json(package_name: &str) -> Result<serde_json::Value> {
    let handlebars_registry = handlebars::Handlebars::new();
    let json_url = handlebars_registry.render_template(
        "https://registry.npmjs.com/{{package_name}}",
        &maplit::btreemap! {"package_name" => package_name},
    )?;

    let mut result = reqwest::blocking::get(&json_url.to_string())?;
    let mut body = String::new();
    result.read_to_string(&mut body)?;

    Ok(serde_json::from_str(&body).context(format!("JSON was not well-formatted:\n{}", body))?)
}

fn get_archive_url(
    registry_entry_json: &serde_json::Value,
    package_version: &str,
) -> Result<url::Url> {
    Ok(url::Url::parse(
        registry_entry_json["versions"][package_version]["dist"]["tarball"]
            .as_str()
            .ok_or(format_err!("Failed to parse package archive URL."))?,
    )?)
}

/// Package dependency file types.
#[derive(Debug, Copy, Clone, strum_macros::EnumIter)]
enum DependencyFileType {
    Npm,
}

impl DependencyFileType {
    /// Return file name associated with dependency type.
    pub fn file_name(&self) -> std::path::PathBuf {
        match self {
            Self::Npm => std::path::PathBuf::from("package-lock.json"),
        }
    }
}

/// Package dependency file type and file path.
#[derive(Debug, Clone)]
struct DependencyFile {
    r#type: DependencyFileType,
    path: std::path::PathBuf,
}

/// Returns a vector of identified package dependency definition files.
///
/// Walks up the directory tree directory tree until the first positive result is found.
fn identify_dependency_files(
    working_directory: &std::path::PathBuf,
) -> Option<Vec<DependencyFile>> {
    assert!(working_directory.is_absolute());
    let mut working_directory = working_directory.clone();

    loop {
        // If at least one target is found, assume package is present.
        let mut found_dependency_file = false;

        let mut dependency_files: Vec<DependencyFile> = Vec::new();
        for dependency_file_type in DependencyFileType::iter() {
            let target_absolute_path = working_directory.join(dependency_file_type.file_name());
            if target_absolute_path.is_file() {
                found_dependency_file = true;
                dependency_files.push(DependencyFile {
                    r#type: dependency_file_type,
                    path: target_absolute_path,
                })
            }
        }
        if found_dependency_file {
            return Some(dependency_files);
        }

        // No need to move further up the directory tree after this loop.
        if working_directory == std::path::PathBuf::from("/") {
            break;
        }

        // Move further up the directory tree.
        working_directory.pop();
    }
    None
}
