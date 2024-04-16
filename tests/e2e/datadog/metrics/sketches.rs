use std::collections::BTreeMap;

#[allow(warnings, clippy::pedantic, clippy::nursery)]
mod ddmetric_proto {
    include!(concat!(env!("OUT_DIR"), "/datadog.agentpayload.rs"));
}

use ddmetric_proto::{
    sketch_payload::sketch::{Distribution, Dogsketch},
    SketchPayload,
};
use tracing::info;

use super::*;

const SKETCHES_ENDPOINT: &str = "/api/beta/sketches";

// unique identification of a Sketch
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
struct SketchContext {
    metric_name: String,
    tags: Vec<String>,
}

type TimeSketchData<T> = BTreeMap<i64, Vec<T>>;

/// This type represents the massaged intake data collected from the upstream.
/// The idea is to be able to store what was received in a way that allows us to
/// compare what is important to compare, and accounting for the bits that are not
/// guaranteed to line up.
///
/// For instance, the services that are running, may start at different times, thus the
/// timestamps for data points received are not guaranteed to match up.
type SketchIntake =
    BTreeMap<SketchContext, (TimeSketchData<Dogsketch>, TimeSketchData<Distribution>)>;

// massages the raw payloads into our intake structure
fn generate_sketch_intake(mut payloads: Vec<SketchPayload>) -> SketchIntake {
    let mut intake = SketchIntake::new();

    payloads.iter_mut().for_each(|payload| {
        payload.sketches.iter_mut().for_each(|sketch| {
            // filter out the metrics we don't care about (ones not generated by the client)
            if !sketch.metric.starts_with("foo_metric") {
                return;
            }
            let ctx = SketchContext {
                metric_name: sketch.metric.clone(),
                tags: sketch.tags.clone(),
            };

            if !intake.contains_key(&ctx) {
                intake.insert(ctx.clone(), (TimeSketchData::new(), TimeSketchData::new()));
            }
            let entry: &mut (TimeSketchData<Dogsketch>, TimeSketchData<Distribution>) =
                intake.get_mut(&ctx).unwrap();

            sketch.dogsketches.iter_mut().for_each(|ds| {
                let ts = ds.ts;
                entry.0.entry(ts).or_default();
                ds.ts = 0;
                entry.0.get_mut(&ts).unwrap().push(ds.clone());
            });

            sketch.distributions.iter_mut().for_each(|dt| {
                let ts = dt.ts;
                entry.1.entry(ts).or_default();
                dt.ts = 0;
                entry.1.get_mut(&ts).unwrap().push(dt.clone());
            });
        });
    });

    intake
}

// runs assertions that each set of payloads should be true to regardless
// of the pipeline
fn common_sketch_assertions(sketches: &SketchIntake) {
    // we should have received some metrics from the emitter
    assert!(!sketches.is_empty());
    info!("metric sketch received: {}", sketches.len());

    let mut found = false;
    sketches.keys().for_each(|ctx| {
        if ctx.metric_name.starts_with("foo_metric.distribution") {
            found = true;
        }
    });

    assert!(found, "Didn't receive metric type distribution");
}

async fn get_sketches_from_pipeline(address: String) -> SketchIntake {
    info!("getting sketch payloads");
    let payloads =
        get_fakeintake_payloads::<FakeIntakeResponseRaw>(&address, SKETCHES_ENDPOINT).await;

    info!("unpacking payloads");
    let payloads = unpack_proto_payloads(&payloads);

    info!("generating sketch intake");
    let sketches = generate_sketch_intake(payloads);

    common_sketch_assertions(&sketches);

    info!("{:?}", sketches);

    sketches
}

pub(super) async fn validate() {
    info!("==== getting sketch data from agent-only pipeline ==== ");
    let agent_sketches = get_sketches_from_pipeline(fake_intake_agent_address()).await;

    info!("==== getting sketch data from agent-vector pipeline ====");
    let vector_sketches = get_sketches_from_pipeline(fake_intake_vector_address()).await;

    agent_sketches
        .iter()
        .zip(vector_sketches.iter())
        .for_each(|(agent_s, vector_s)| {
            assert_eq!(agent_s.0, vector_s.0, "Mismatch of sketch context");

            assert_eq!(agent_s.1, vector_s.1, "Mismatch of sketch data");
        });
}
