// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use self::framework::FrameworkReleaseConfig;
use crate::{
    aptos_core_path, aptos_framework_path, components::feature_flags::Features,
    release_builder_path,
};
use anyhow::{anyhow, bail, Context, Result};
use aptos::governance::GenerateExecutionHash;
use aptos_rest_client::Client;
use aptos_temppath::TempPath;
use aptos_types::{
    account_config::CORE_CODE_ADDRESS,
    on_chain_config::{
        ExecutionConfigV1, GasScheduleV2, OnChainConfig, OnChainConsensusConfig,
        OnChainExecutionConfig, TransactionShufflerType, Version,
    },
};
use futures::executor::block_on;
use handlebars::Handlebars;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use url::Url;

pub mod consensus_config;
pub mod execution_config;
pub mod feature_flags;
pub mod framework;
pub mod gas;
pub mod transaction_fee;
pub mod version;

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct ReleaseConfig {
    pub name: String,
    pub remote_endpoint: Option<Url>,
    pub proposals: Vec<Proposal>,
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq)]
pub struct Proposal {
    pub name: String,
    pub metadata: ProposalMetadata,
    pub execution_mode: ExecutionMode,
    pub update_sequence: Vec<ReleaseEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ProposalMetadata {
    title: String,
    description: String,
    #[serde(default = "default_url")]
    source_code_url: String,
    #[serde(default = "default_url")]
    discussion_url: String,
}

fn default_url() -> String {
    "https://github.com/aptos-labs/aptos-core".to_string()
}

#[derive(Serialize, Deserialize, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionMode {
    MultiStep,
    SingleStep,
    RootSigner,
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
pub enum ReleaseEntry {
    Framework(FrameworkReleaseConfig),
    CustomGas(GasScheduleV2),
    DefaultGas,
    Version(Version),
    FeatureFlag(Features),
    Consensus(OnChainConsensusConfig),
    Execution(OnChainExecutionConfig),
    RawScript(PathBuf),
}

impl ReleaseEntry {
    pub fn generate_release_script(
        &self,
        client: Option<&Client>,
        result: &mut Vec<(String, String)>,
        execution_mode: ExecutionMode,
    ) -> Result<()> {
        let (is_testnet, is_multi_step) = match execution_mode {
            ExecutionMode::MultiStep => (false, true),
            ExecutionMode::SingleStep => (false, false),
            ExecutionMode::RootSigner => (true, false),
        };
        match self {
            ReleaseEntry::Framework(framework_release) => {
                result.append(
                    &mut framework::generate_upgrade_proposals(
                        framework_release,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )
                    .unwrap(),
                );
            },
            ReleaseEntry::CustomGas(gas_schedule) => {
                if !fetch_and_equals::<GasScheduleV2>(client, gas_schedule)? {
                    result.append(&mut gas::generate_gas_upgrade_proposal(
                        gas_schedule,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )?);
                }
            },
            ReleaseEntry::DefaultGas => {
                let gas_schedule = aptos_gas::gen::current_gas_schedule();
                if !fetch_and_equals::<GasScheduleV2>(client, &gas_schedule)? {
                    result.append(&mut gas::generate_gas_upgrade_proposal(
                        &gas_schedule,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )?);
                }
            },
            ReleaseEntry::Version(version) => {
                if !fetch_and_equals::<Version>(client, version)? {
                    result.append(&mut version::generate_version_upgrade_proposal(
                        version,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )?);
                }
            },
            ReleaseEntry::FeatureFlag(feature_flags) => {
                let mut needs_update = true;
                if let Some(client) = client {
                    let features = block_on(async {
                        client
                            .get_account_resource_bcs::<aptos_types::on_chain_config::Features>(
                                CORE_CODE_ADDRESS,
                                "0x1::features::Features",
                            )
                            .await
                    })?;
                    // Only update the feature flags section when there's a divergence between the local configs and on chain configs.
                    // If any flag in the release config diverges from the on chain value, we will emit a script that includes all flags
                    // we would like to enable/disable, regardless of their current on chain state.
                    needs_update = feature_flags.has_modified(features.inner());
                }
                if needs_update {
                    result.append(&mut feature_flags::generate_feature_upgrade_proposal(
                        feature_flags,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )?);
                }
            },
            ReleaseEntry::Consensus(consensus_config) => {
                if !fetch_and_equals(client, consensus_config)? {
                    result.append(&mut consensus_config::generate_consensus_upgrade_proposal(
                        consensus_config,
                        is_testnet,
                        if is_multi_step {
                            get_execution_hash(result)
                        } else {
                            "".to_owned().into_bytes()
                        },
                    )?);
                }
            },
            ReleaseEntry::Execution(execution_config) => {
                if !fetch_and_equals(client, execution_config)? {
                    result.append(
                        &mut execution_config::generate_execution_config_upgrade_proposal(
                            execution_config,
                            is_testnet,
                            if is_multi_step {
                                get_execution_hash(result)
                            } else {
                                "".to_owned().into_bytes()
                            },
                        )?,
                    );
                }
            },
            ReleaseEntry::RawScript(script_path) => {
                let base_path = aptos_core_path().join(script_path.as_path());
                let file_name = base_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        anyhow!("Unable to obtain file name for proposal: {:?}", script_path)
                    })?
                    .to_string();
                let file_content = std::fs::read_to_string(base_path)
                    .with_context(|| format!("Unable to read file: {}", script_path.display()))?;

                if let ExecutionMode::MultiStep = execution_mode {
                    // Render the hash for multi step proposal.
                    // {{ script_hash }} in the provided move file will be replaced with the real hash.

                    let mut handlebars = Handlebars::new();
                    handlebars
                        .register_template_string("move_template", file_content.as_str())
                        .unwrap();

                    let execution_hash = get_execution_hash(result);
                    let mut hash_string = "vector[".to_string();
                    for b in execution_hash.iter() {
                        hash_string.push_str(format!("{}u8,", b).as_str());
                    }
                    hash_string.push(']');

                    let mut data = HashMap::new();
                    data.insert("script_hash", hash_string);

                    result.push((
                        file_name,
                        handlebars
                            .render("move_template", &data)
                            .map_err(|err| anyhow!("Fail to render string: {:?}", err))?,
                    ));
                } else {
                    result.push((file_name, file_content));
                }
            },
        }
        Ok(())
    }

    pub fn validate_upgrade(&self, client: &Client) -> Result<()> {
        let client_opt = Some(client);
        match self {
            ReleaseEntry::Framework(_) => (),
            ReleaseEntry::RawScript(_) => (),
            ReleaseEntry::CustomGas(gas_schedule) => {
                if !fetch_and_equals(client_opt, gas_schedule)? {
                    bail!("Gas schedule config mismatch: Expected {:?}", gas_schedule);
                }
            },
            ReleaseEntry::DefaultGas => {
                if !fetch_and_equals(client_opt, &aptos_gas::gen::current_gas_schedule())? {
                    bail!("Gas schedule config mismatch: Expected Default");
                }
            },
            ReleaseEntry::Version(version) => {
                if !fetch_and_equals(client_opt, version)? {
                    bail!("Version config mismatch: Expected {:?}", version);
                }
            },
            ReleaseEntry::FeatureFlag(features) => {
                let on_chain_features = block_on(async {
                    client
                        .get_account_resource_bcs::<aptos_types::on_chain_config::Features>(
                            CORE_CODE_ADDRESS,
                            "0x1::features::Features",
                        )
                        .await
                })?;
                if features.has_modified(on_chain_features.inner()) {
                    bail!(
                        "Feature mismatch: Got {:?}, expected {:?}",
                        on_chain_features.inner(),
                        features
                    );
                }
            },
            ReleaseEntry::Consensus(consensus_config) => {
                if !fetch_and_equals(client_opt, consensus_config)? {
                    bail!("Consensus config mismatch: Expected {:?}", consensus_config);
                }
            },
            ReleaseEntry::Execution(execution_config) => {
                if !fetch_and_equals(client_opt, execution_config)? {
                    bail!("Consensus config mismatch: Expected {:?}", execution_config);
                }
            },
        }
        Ok(())
    }
}

// Compare the current on chain config with the value recorded on chain. Return false if there's a difference.
fn fetch_and_equals<T: OnChainConfig + PartialEq>(
    client: Option<&Client>,
    expected: &T,
) -> Result<bool> {
    match client {
        Some(client) => {
            let config = T::deserialize_into_config(
                block_on(async {
                    client
                        .get_account_resource_bytes(
                            CORE_CODE_ADDRESS,
                            format!(
                                "{}::{}::{}",
                                T::ADDRESS,
                                T::MODULE_IDENTIFIER,
                                T::TYPE_IDENTIFIER
                            )
                            .as_str(),
                        )
                        .await
                })?
                .inner(),
            )?;

            Ok(&config == expected)
        },
        None => Ok(false),
    }
}

impl ReleaseConfig {
    pub fn generate_release_proposal_scripts(&self, base_path: &Path) -> Result<()> {
        let client = self
            .remote_endpoint
            .as_ref()
            .map(|url| Client::new(url.clone()));

        // Create directories for source and metadata.
        let mut source_dir = base_path.to_path_buf();

        // If source dir doesnt exist create it, if it does exist error
        if !source_dir.exists() {
            println!("Creating source directory: {:?}", source_dir);
            std::fs::create_dir(source_dir.as_path()).map_err(|err| {
                anyhow!(
                    "Fail to create folder for source: {} {:?}",
                    source_dir.display(),
                    err
                )
            })?;
        }

        source_dir.push("sources");

        std::fs::create_dir(source_dir.as_path())
            .map_err(|err| anyhow!("Fail to create folder for source: {:?}", err))?;

        source_dir.push(&self.name);
        std::fs::create_dir(source_dir.as_path())
            .map_err(|err| anyhow!("Fail to create folder for source: {:?}", err))?;

        let mut metadata_dir = base_path.to_path_buf();
        metadata_dir.push("metadata");

        std::fs::create_dir(metadata_dir.as_path())
            .map_err(|err| anyhow!("Fail to create folder for metadata: {:?}", err))?;
        metadata_dir.push(&self.name);
        std::fs::create_dir(metadata_dir.as_path())
            .map_err(|err| anyhow!("Fail to create folder for metadata: {:?}", err))?;

        // If we are generating multi-step proposal files, we generate the files in reverse order,
        // since we need to pass in the hash of the next file to the previous file.
        for proposal in &self.proposals {
            let mut proposal_dir = base_path.to_path_buf();
            proposal_dir.push("sources");
            proposal_dir.push(&self.name);
            proposal_dir.push(proposal.name.as_str());

            std::fs::create_dir(proposal_dir.as_path())
                .map_err(|err| anyhow!("Fail to create folder for proposal: {:?}", err))?;

            let mut result: Vec<(String, String)> = vec![];
            if let ExecutionMode::MultiStep = &proposal.execution_mode {
                for entry in proposal.update_sequence.iter().rev() {
                    entry.generate_release_script(
                        client.as_ref(),
                        &mut result,
                        proposal.execution_mode,
                    )?;
                }
                result.reverse();
            } else {
                for entry in proposal.update_sequence.iter() {
                    entry.generate_release_script(
                        client.as_ref(),
                        &mut result,
                        proposal.execution_mode,
                    )?;
                }
            }

            for (idx, (script_name, script)) in result.into_iter().enumerate() {
                let mut script_path = proposal_dir.clone();
                let proposal_name = format!("{}-{}", idx, script_name);
                script_path.push(&proposal_name);
                script_path.set_extension("move");

                std::fs::write(script_path.as_path(), append_script_hash(script).as_bytes())
                    .map_err(|err| anyhow!("Failed to write to file: {:?}", err))?;
            }

            let mut metadata_path = base_path.to_path_buf();
            metadata_path.push("metadata");
            metadata_path.push(proposal.name.as_str());
            metadata_path.set_extension("json");

            std::fs::write(
                metadata_path.as_path(),
                serde_json::to_string_pretty(&proposal.metadata)?,
            )
            .map_err(|err| anyhow!("Failed to write to file: {:?}", err))?;
        }

        Ok(())
    }

    pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Self> {
        // Open the file and read it into a string
        let config_path_string = path.as_ref().to_str().unwrap().to_string();
        let mut file = File::open(&path).map_err(|error| {
            anyhow!(
                "Failed to open config file: {:?}. Error: {:?}",
                config_path_string,
                error
            )
        })?;
        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(|error| {
            anyhow!(
                "Failed to read the config file into a string: {:?}. Error: {:?}",
                config_path_string,
                error
            )
        })?;

        // Parse the file string
        Self::parse(&contents)
    }

    pub fn save_config<P: AsRef<Path>>(&self, output_file: P) -> Result<()> {
        let contents =
            serde_yaml::to_vec(&self).map_err(|e| anyhow!("failed to generate config: {:?}", e))?;
        let mut file = File::create(output_file.as_ref())
            .map_err(|e| anyhow!("failed to create file: {:?}", e))?;
        file.write_all(&contents)
            .map_err(|e| anyhow!("failed to write file: {:?}", e))?;
        Ok(())
    }

    pub fn parse(serialized: &str) -> Result<Self> {
        serde_yaml::from_str(serialized).map_err(|e| anyhow!("Failed to parse the config: {:?}", e))
    }

    // Fetch all configs from a remote rest endpoint and assert all the configs are the same as the ones specified locally.
    pub fn validate_upgrade(&self, endpoint: Url) -> Result<()> {
        let client = Client::new(endpoint);
        for proposal in &self.proposals {
            for entry in &proposal.update_sequence {
                entry.validate_upgrade(&client)?;
            }
        }
        Ok(())
    }
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        ReleaseConfig {
            name: "TestingConfig".to_string(),
            remote_endpoint: None,
            proposals: vec![
                Proposal {
                    execution_mode: ExecutionMode::SingleStep,
                    metadata: ProposalMetadata::default(),
                    name: "custom".to_string(),
                    update_sequence: vec![ReleaseEntry::RawScript(PathBuf::from(
                        "data/proposals/empty.move",
                    ))],
                },
                Proposal {
                    execution_mode: ExecutionMode::MultiStep,
                    metadata: ProposalMetadata::default(),
                    name: "framework".to_string(),
                    update_sequence: vec![ReleaseEntry::Framework(FrameworkReleaseConfig {
                        bytecode_version: 6, // TODO: remove explicit bytecode version from sources
                        git_hash: None,
                    })],
                },
                Proposal {
                    execution_mode: ExecutionMode::MultiStep,
                    metadata: ProposalMetadata::default(),
                    name: "gas".to_string(),
                    update_sequence: vec![ReleaseEntry::DefaultGas],
                },
                Proposal {
                    execution_mode: ExecutionMode::MultiStep,
                    metadata: ProposalMetadata::default(),
                    name: "feature_flags".to_string(),
                    update_sequence: vec![
                        ReleaseEntry::FeatureFlag(Features {
                            enabled: aptos_vm_genesis::default_features()
                                .into_iter()
                                .map(crate::components::feature_flags::FeatureFlag::from)
                                .collect(),
                            disabled: vec![],
                        }),
                        ReleaseEntry::Consensus(OnChainConsensusConfig::default()),
                        ReleaseEntry::Execution(OnChainExecutionConfig::V1(ExecutionConfigV1 {
                            transaction_shuffler_type: TransactionShufflerType::SenderAwareV1(32),
                        })),
                        ReleaseEntry::RawScript(PathBuf::from(
                            "data/proposals/empty_multi_step.move",
                        )),
                    ],
                },
            ],
        }
    }
}

pub fn get_execution_hash(result: &Vec<(String, String)>) -> Vec<u8> {
    if result.is_empty() {
        "vector::empty<u8>()".to_owned().into_bytes()
    } else {
        let temp_script_path = TempPath::new();
        temp_script_path.create_as_file().unwrap();
        let mut move_script_path = temp_script_path.path().to_path_buf();
        move_script_path.set_extension("move");
        std::fs::write(move_script_path.as_path(), result.last().unwrap().1.clone())
            .map_err(|err| {
                anyhow!(
                    "Failed to get execution hash: failed to write to file: {:?}",
                    err
                )
            })
            .unwrap();

        let (_, hash) = GenerateExecutionHash {
            script_path: Option::from(move_script_path),
            framework_local_dir: Some(aptos_framework_path()),
        }
        .generate_hash()
        .unwrap();
        hash.to_vec()
    }
}

fn append_script_hash(raw_script: String) -> String {
    let temp_script_path = TempPath::new();
    temp_script_path.create_as_file().unwrap();

    let mut move_script_path = temp_script_path.path().to_path_buf();
    move_script_path.set_extension("move");
    std::fs::write(move_script_path.as_path(), raw_script.as_bytes())
        .map_err(|err| {
            anyhow!(
                "Failed to get execution hash: failed to write to file: {:?}",
                err
            )
        })
        .unwrap();

    let (_, hash) = GenerateExecutionHash {
        script_path: Option::from(move_script_path),
        framework_local_dir: Some(aptos_framework_path()),
    }
    .generate_hash()
    .unwrap();

    format!("// Script hash: {} \n{}", hash, raw_script)
}

impl Default for ProposalMetadata {
    fn default() -> Self {
        ProposalMetadata {
            title: "default".to_string(),
            description: "default".to_string(),
            // Aptos CLI need a valid url for the two fields.
            source_code_url: default_url(),
            discussion_url: default_url(),
        }
    }
}
