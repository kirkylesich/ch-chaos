use kube::CustomResourceExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let experiment_crd = chimp_chaos::operator::crd::ChaosExperiment::crd();
    let analysis_crd = chimp_chaos::operator::crd::ChaosAnalysis::crd();

    print!(
        "{}---\n{}",
        serde_yaml::to_string(&experiment_crd)?,
        serde_yaml::to_string(&analysis_crd)?
    );
    Ok(())
}
