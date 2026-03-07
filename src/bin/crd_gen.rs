use kube::CustomResourceExt;

fn main() {
    let experiment_crd = chimp_chaos::operator::crd::ChaosExperiment::crd();
    let analysis_crd = chimp_chaos::operator::crd::ChaosAnalysis::crd();

    print!(
        "{}---\n{}",
        serde_yaml::to_string(&experiment_crd).unwrap(),
        serde_yaml::to_string(&analysis_crd).unwrap()
    );
}
