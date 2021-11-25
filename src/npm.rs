use anyhow::{Context, Result};
use std::collections::HashSet;

static HOST_NAME: &str = "npmjs.com";

/// Parse and clean package version string.
///
/// Returns a structure which details common errors.
fn get_parsed_version(version: &Option<&str>) -> vouch_lib::extension::common::VersionParseResult {
    if let Some(version) = version.and_then(|v| Some(v.to_string())) {
        if version != "" {
            return Ok(version);
        }
    }
    Err(vouch_lib::extension::common::VersionError::from_missing_version())
}

type JsonObject = serde_json::Map<String, serde_json::Value>;

fn parse_dependencies(
    package_entry: &serde_json::Value,
    include_dev_dependencies: bool,
) -> Result<Vec<vouch_lib::extension::Dependency>> {
    let mut unprocessed_dependencies_sections: std::collections::VecDeque<&JsonObject> =
        std::collections::VecDeque::new();

    if let Some(dependencies) = package_entry["dependencies"].as_object() {
        unprocessed_dependencies_sections.push_back(dependencies);
    }

    let mut all_dependencies = HashSet::new();
    while let Some(dependencies) = unprocessed_dependencies_sections.pop_front() {
        for (package_name, entry) in dependencies {
            if !include_dev_dependencies && entry["dev"].as_bool().unwrap_or_default() {
                continue;
            }

            let version_parse_result = get_parsed_version(&entry["version"].as_str());
            all_dependencies.insert(vouch_lib::extension::Dependency {
                name: package_name.clone(),
                version: version_parse_result,
            });

            if let Some(sub_dependencies) = entry["dependencies"].as_object() {
                unprocessed_dependencies_sections.push_back(sub_dependencies);
            }
        }
    }

    let mut all_dependencies: Vec<_> = all_dependencies.into_iter().collect();
    all_dependencies.sort();
    Ok(all_dependencies)
}

/// Parse dependencies from project dependencies definition file.
pub fn get_dependencies(
    file_path: &std::path::PathBuf,
    include_dev_dependencies: bool,
) -> Result<Vec<vouch_lib::extension::Dependency>> {
    let file = std::fs::File::open(file_path)?;
    let reader = std::io::BufReader::new(file);
    let package_entry: serde_json::Value = serde_json::from_reader(reader).context(format!(
        "Failed to parse package-lock.json: {}",
        file_path.display()
    ))?;

    let all_dependencies = parse_dependencies(&package_entry, include_dev_dependencies)?;
    Ok(all_dependencies)
}

pub fn get_registry_host_name() -> String {
    HOST_NAME.to_string()
}
