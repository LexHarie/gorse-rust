// Ports: model/ctr/model_test.go @ add21b5

use std::io::Cursor;

use gorse_common::floats::from_float32;
use gorse_common::gocontext::Context;
use gorse_dataset::index::Index;
use gorse_dataset::unified_index::UnifiedMapIndexBuilder;
use gorse_model_core::model::Model;
use gorse_model_core::params::{
    AUTO_SCALE, BATCH_SIZE, INIT_STD_DEV, LR, N_EPOCHS, N_FACTORS, ParamValue, Params, REG,
};

use super::Afm;
use crate::ctr::data::{Dataset, Embedding, Label, load_data_from_built_in};
use crate::ctr::evaluator::evaluate_classification;
use crate::ctr::model::{
    BatchInference, FactorizationMachines, FitConfig, marshal_model, unmarshal_model,
};
use crate::ctr::new_map_index_dataset;

const CLASSIFICATION_DELTA: f32 = 0.01;

fn params(entries: Vec<(&str, ParamValue)>) -> Params {
    Params(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

fn fit_config_with_test_tracker() -> FitConfig {
    let mut config = FitConfig::new();
    config
        .set_verbose(1)
        .set_jobs(std::thread::available_parallelism().map_or(1, |n| n.get()));
    config
}

fn label(name: &str, value: f32) -> Label {
    Label {
        name: name.to_string(),
        value,
    }
}

fn input(
    user: &str,
    item: &str,
    user_labels: Vec<Label>,
    item_labels: Vec<Label>,
) -> (String, String, Vec<Label>, Vec<Label>) {
    (user.to_string(), item.to_string(), user_labels, item_labels)
}

#[test]
#[ignore = "O-1: dataset from cdn.gorse.io"]
fn factorization_machines_classification_frappe() {
    // python .\model.py frappe -dim 8 -iter 10 -learn_rate 0.01 -regular 0.0001
    let (train, test) = load_data_from_built_in("frappe").expect("frappe");
    let mut m = Afm::new(params(vec![
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.01)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
    ]));
    let fit_config = fit_config_with_test_tracker();
    let score = m.fit(&Context::background(), &train, &test, &fit_config);
    assert!(score.accuracy >= 0.919 - CLASSIFICATION_DELTA);

    let mut buf = Vec::new();
    marshal_model(&mut buf, &m).expect("marshal");
    let tmp = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");
    let score_clone = evaluate_classification(&*tmp, &test, fit_config.jobs);
    assert!(score_clone.accuracy >= 0.919 - CLASSIFICATION_DELTA);
}

#[test]
#[ignore = "O-1: dataset from cdn.gorse.io (ml-tag). Go skips this test as time-consuming"]
fn factorization_machines_classification_movie_lens() {
    // python .\model.py ml-tag -dim 8 -iter 10 -learn_rate 0.01 -regular 0.0001
    let (train, test) = load_data_from_built_in("ml-tag").expect("ml-tag");
    let mut m = Afm::new(params(vec![
        (INIT_STD_DEV, ParamValue::Float64(0.01)),
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.001)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
    ]));
    let fit_config = fit_config_with_test_tracker();
    let score = m.fit(&Context::background(), &train, &test, &fit_config);
    assert!((0.815 - score.accuracy).abs() <= CLASSIFICATION_DELTA);
}

#[test]
#[ignore = "O-1: dataset from cdn.gorse.io (criteo)"]
fn factorization_machines_classification_criteo() {
    // python .\model.py criteo -dim 8 -iter 10 -learn_rate 0.01 -regular 0.0001
    let (train, test) = load_data_from_built_in("criteo").expect("criteo");
    let mut m = Afm::new(params(vec![
        (N_FACTORS, ParamValue::Int(8)),
        (N_EPOCHS, ParamValue::Int(10)),
        (LR, ParamValue::Float64(0.01)),
        (REG, ParamValue::Float64(0.0001)),
        (BATCH_SIZE, ParamValue::Int(1024)),
        (AUTO_SCALE, ParamValue::Bool(false)),
    ]));
    let fit_config = fit_config_with_test_tracker();
    let score = m.fit(&Context::background(), &train, &test, &fit_config);
    assert!((0.77 - score.accuracy).abs() <= 0.025);

    // test prediction
    let inputs = vec![(vec![1, 2, 3, 4, 5, 6], vec![1.0, 1.0, 0.3, 0.4, 0.5, 0.6])];
    assert_eq!(
        m.batch_internal_predict(&inputs, &[Vec::new(), Vec::new()], fit_config.jobs),
        m.batch_predict(
            &[input(
                "1",
                "2",
                vec![label("3", 0.3), label("4", 0.4)],
                vec![label("5", 0.5), label("6", 0.6)],
            )],
            &[Vec::new(), Vec::new()],
            fit_config.jobs,
        )
    );

    // test marshal and unmarshal
    let mut buf = Vec::new();
    marshal_model(&mut buf, &m).expect("marshal");
    let tmp = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");
    let score_clone = evaluate_classification(&*tmp, &test, fit_config.jobs);
    assert!((0.77 - score_clone.accuracy).abs() <= 0.02);

    // test clear
    assert!(!m.invalid());
    m.clear();
    assert!(m.invalid());
}

fn new_synthesis_dataset() -> Dataset {
    let mut builder = UnifiedMapIndexBuilder::new();
    builder.add_user("u0");
    builder.add_user("u1");
    builder.add_user_label("ul0");
    builder.add_user_label("ul1");
    builder.add_user_label("ul2");
    builder.add_item("i0");
    builder.add_item("i1");
    builder.add_item_label("il0");
    builder.add_item_label("il1");
    builder.add_item_label("il2");

    let mut data_set = new_map_index_dataset();
    data_set.index = Some(builder.build());
    data_set.user_labels = vec![
        vec![(0, 1.0), (1, 0.5), (2, -1.0)],
        vec![(0, -1.0), (1, -0.5), (2, 1.0)],
    ];
    data_set.item_labels = vec![
        vec![(0, 1.0), (1, 0.5), (2, -1.0)],
        vec![(0, -1.0), (1, -0.5), (2, 1.0)],
    ];
    let mut embedding_index = Index::new();
    embedding_index.add("e1");
    embedding_index.add("e2");
    data_set.item_embedding_index = Some(embedding_index);
    data_set.item_embedding_dimension = vec![3, 4];
    data_set.item_embeddings = vec![
        vec![
            from_float32(&[0.8, 0.8, 0.8]),
            from_float32(&[0.1, 0.1, 0.1, 0.1]),
        ],
        vec![
            from_float32(&[-0.8, -0.8, -0.8]),
            from_float32(&[-0.1, -0.1, -0.1, -0.1]),
        ],
    ];

    data_set.users = vec![0, 0, 1, 1];
    data_set.items = vec![0, 1, 0, 1];
    data_set.target = vec![1.0, -1.0, -1.0, 1.0];
    data_set.positive_count = 2;
    data_set.negative_count = 2;
    data_set
}

#[test]
fn factorization_machines_classification_synthesis() {
    let data_set = new_synthesis_dataset();
    let fit_config = fit_config_with_test_tracker();
    let mut m = Afm::new(Params::default());
    let score = m.fit(&Context::background(), &data_set, &data_set, &fit_config);
    assert!(score.accuracy >= 0.5);

    let mut buf = Vec::new();
    marshal_model(&mut buf, &m).expect("marshal");
    let clone = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");
    let clone_score = evaluate_classification(&*clone, &data_set, fit_config.jobs);
    assert!((score.accuracy - clone_score.accuracy).abs() <= 0.05);

    let (indices_pos, values_pos, embeddings_pos, _) = data_set.get(0);
    let (indices_neg, values_neg, embeddings_neg, _) = data_set.get(1);
    assert_eq!(
        m.batch_internal_predict(
            &[(indices_pos, values_pos), (indices_neg, values_neg)],
            &[embeddings_pos.clone(), embeddings_neg.clone()],
            fit_config.jobs,
        ),
        m.batch_predict(
            &[
                input(
                    "u0",
                    "i0",
                    vec![label("ul0", 1.0), label("ul1", 0.5), label("ul2", -1.0)],
                    vec![label("il0", 1.0), label("il1", 0.5), label("il2", -1.0)],
                ),
                input(
                    "u0",
                    "i1",
                    vec![label("ul0", 1.0), label("ul1", 0.5), label("ul2", -1.0)],
                    vec![label("il0", -1.0), label("il1", -0.5), label("il2", 1.0)],
                ),
            ],
            &[
                vec![
                    Embedding {
                        name: "e1".to_string(),
                        value: embeddings_pos[0].clone(),
                    },
                    Embedding {
                        name: "e2".to_string(),
                        value: embeddings_pos[1].clone(),
                    },
                ],
                vec![
                    Embedding {
                        name: "e1".to_string(),
                        value: embeddings_neg[0].clone(),
                    },
                    Embedding {
                        name: "e2".to_string(),
                        value: embeddings_neg[1].clone(),
                    },
                ],
            ],
            fit_config.jobs,
        )
    );

    assert_eq!(
        m.batch_predict(
            &[
                input("u0", "i0", Vec::new(), Vec::new()),
                input("u0", "i1", Vec::new(), Vec::new()),
                input(
                    "u0",
                    "i0",
                    vec![label("ul_unknown", 1.0)],
                    vec![label("il_unknown", 1.0)],
                ),
                input(
                    "u0",
                    "i1",
                    vec![label("ul_unknown", 1.0)],
                    vec![label("il_unknown", 1.0)],
                ),
            ],
            &[
                Vec::new(),
                Vec::new(),
                vec![Embedding {
                    name: "unknown_embedding".to_string(),
                    value: vec![0; 3],
                }],
                vec![Embedding {
                    name: "unknown_embedding".to_string(),
                    value: vec![0; 3],
                }],
            ],
            fit_config.jobs,
        )
        .len(),
        4
    );
}

#[test]
fn factorization_machines_no_embeddings() {
    let mut data_set = new_synthesis_dataset();
    data_set.item_embedding_index = Some(Index::new());
    data_set.item_embedding_dimension = Vec::new();
    data_set.item_embeddings = Vec::new();

    let mut m = Afm::new(Params::default());
    m.init(&data_set);

    let mut buf = Vec::new();
    marshal_model(&mut buf, &m).expect("marshal");
    let clone = unmarshal_model(&mut Cursor::new(buf)).expect("unmarshal");

    let batch = clone.as_batch_inference().expect("batch inference");
    let inputs = vec![input("u0", "i0", Vec::new(), Vec::new())];
    assert_eq!(
        batch.batch_predict(&inputs, &[Vec::new()], 1),
        batch.batch_predict(
            &inputs,
            &[vec![Embedding {
                name: "unexpected".to_string(),
                value: vec![1],
            }]],
            1,
        )
    );
}

#[test]
fn factorization_machines_predict_with_more_features_than_training() {
    let mut data_set = new_synthesis_dataset();
    // Only the first user-item pair is present in the training set, while the index
    // also contains users and items with more labels.
    data_set.user_labels[0].truncate(1);
    data_set.item_labels[0].truncate(1);
    data_set.users = vec![0];
    data_set.items = vec![0];
    data_set.target = vec![1.0];

    let mut m = Afm::new(Params::default());
    m.init(&data_set);
    assert_eq!(m.num_dimension, 4);

    let inputs = vec![
        input("u0", "i0", vec![label("ul0", 1.0)], vec![label("il0", 1.0)]),
        input(
            "u1",
            "i1",
            vec![label("ul0", 1.0), label("ul1", 1.0), label("ul2", 1.0)],
            vec![label("il0", 1.0), label("il1", 1.0), label("il2", 1.0)],
        ),
    ];
    let embeddings = vec![Vec::new(); inputs.len()];

    let batch_predictions = m.batch_predict(&inputs, &embeddings, 1);
    assert_eq!(batch_predictions.len(), inputs.len());
    for i in 0..inputs.len() {
        let prediction = m.batch_predict(&inputs[i..i + 1], &embeddings[i..i + 1], 1);
        assert!((prediction[0] - batch_predictions[i]).abs() <= 1e-5);
    }
}

#[test]
fn set_params_reads_the_hyper_parameters_with_go_defaults() {
    let m = Afm::new(Params::default());
    assert_eq!(m.batch_size, 1024);
    assert_eq!(m.n_factors, 16);
    assert_eq!(m.n_epochs, 50);
    assert_eq!(m.lr, 0.001);
    assert_eq!(m.reg, 0.0002);
    assert_eq!(m.init_std_dev, 0.01);
    assert_eq!(m.optimizer, "adam");
    assert!(m.auto_scale);
    assert!(m.invalid());
    assert_eq!(m.header(), "AFM2");
}
