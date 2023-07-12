use anyhow::anyhow;
use clap::Args;
use determinator::{Determinator, DeterminatorSet};
use guppy::{graph::PackageGraph, CargoMetadata, MetadataCommand};

#[derive(Args, Debug, Clone)]
pub struct SelectedPackageArgs {
    #[arg(short, long)]
    pub package: Vec<String>
}

impl SelectedPackageArgs {

}

pub struct ChangeSet {
    current: PackageGraph,
    base: PackageGraph,
}

impl ChangeSet {
    pub fn init() -> anyhow::Result<Self> {
        // Run cargo metadata command
        let current_metadata = MetadataCommand::new()
            .exec()
            .map_err(|e| anyhow!("{}", e))?;
        let current = current_metadata.build_graph().unwrap();

        // Get cargo metadata for HEAD
        // Read local file called metadata.json
        let base_metadata = CargoMetadata::parse_json(include_str!("../../../metadata.json"))
            .map_err(|e| anyhow!("{}", e))?;
        let base = base_metadata.build_graph().unwrap();

        Ok(Self { current, base })
    }

    pub fn determine_changed_packages<'g>(&'g self) -> DeterminatorSet<'g> {
        let mut determinator = Determinator::new(&self.base, &self.current);
        // The determinator expects a list of changed files to be passed in.
        determinator.add_changed_paths(vec!["tools/x/src/common.rs"]);

        determinator.compute()
    }
}
