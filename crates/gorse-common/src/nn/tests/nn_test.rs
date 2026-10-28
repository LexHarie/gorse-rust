// Ports: common/nn/nn_test.go @ add21b5

//! Ports of `common/nn/nn_test.go`: training small models and checking that
//! the loss falls, and that a saved model loads back with the same loss.
//! `TestIris` and `TestMNIST` need datasets that Go downloads (they are
//! ignored, as in `ann`); the Go `TestMNIST` also skips unless it runs on
//! Apple silicon or on a CPU with AVX-512.

use std::time::Instant;

use super::*;
use crate::math32;

/// `accuracy`: the fraction of rows whose largest column is the label.
fn accuracy(prediction: &Tensor, target: &Tensor) -> f32 {
    let labels = target.data();
    let mut precision = 0.0f32;
    for (i, gt) in labels.iter().enumerate() {
        if prediction.slice(i, i + 1).argmax().expect("argmax")[1] == *gt as usize {
            precision += 1.0;
        }
    }
    precision / labels.len() as f32
}

/// `spiral`: three interleaved arms of 100 points each, shuffled.
fn spiral() -> (Tensor, Tensor) {
    let (num_data, num_class, input_dim) = (100, 3, 2);
    let data_size = num_class * num_data;
    let mut x = vec![0.0f32; data_size * input_dim];
    let mut t = vec![0.0f32; data_size];
    for j in 0..num_class {
        for i in 0..num_data {
            let rate = i as f32 / num_data as f32;
            let radius = 1.0 * rate;
            let noise = global(|r| r.norm_float64()) as f32;
            let theta = j as f32 * 4.0 + 4.0 * rate + noise * 0.2;
            let ix = num_data * j + i;
            x[ix * input_dim] = radius * math32::sin(theta);
            x[ix * input_dim + 1] = radius * math32::cos(theta);
            t[ix] = j as f32;
        }
    }
    let indices = global_perm(data_size);
    let x = new_tensor(x, &[data_size, input_dim]).slice_indices(&indices);
    let t = new_tensor(t, &[data_size]).slice_indices(&indices);
    (x, t)
}

#[test]
fn test_linear_regression() {
    let x = rand(None, &[100, 1]);
    let y = add(
        &rand(None, &[100, 1]),
        &[&new_scalar(5.0), &mul(&new_scalar(2.0), &x)],
    );

    let w = zeros(&[1, 1]);
    let b = zeros(&[1]);
    let predict = |x: &Tensor| add(&mat_mul(x, &w, false, false, 0), &[&b]);

    let lr = 0.1f32;
    for _ in 0..100 {
        let y_pred = predict(&x);
        let loss = mean_square_error(&y, &y_pred);

        w.set_grad(None);
        b.set_grad(None);
        loss.backward();

        w.sub(&w.grad().unwrap().mul(&new_scalar(lr)));
        b.sub(&b.grad().unwrap().mul(&new_scalar(lr)));
    }

    assert_eq!(w.shape(), [1, 1]);
    assert_in_delta(f64::from(w.data()[0]), 2.0, 0.6);
    assert_eq!(b.shape(), [1]);
    assert_in_delta(f64::from(b.data()[0]), 5.0, 0.6);
}

#[test]
fn test_neural_network() {
    let mut rng = GoRand::new(0);
    let x = rand(Some(&mut rng), &[100, 1]);
    let y = add(
        &rand(Some(&mut rng), &[100, 1]),
        &[&sin(&mul(&x, &new_scalar(2.0 * math32::PI)))],
    );

    let l1 = new_linear(1, 10);
    let l2 = new_linear(10, 1);
    normal_init(Some(&mut rng), &l1.w, 0.0, 0.01);
    normal_init(Some(&mut rng), &l2.w, 0.0, 0.01);
    let model = new_sequential(vec![Box::new(l1), Box::new(new_sigmoid()), Box::new(l2)]);
    let mut optimizer = new_sgd(model.parameters(), 0.2);

    let mut l = 0.0;
    for _ in 0..10000 {
        let y_pred = model.forward(&x);
        let loss = mean_square_error(&y, &y_pred);

        optimizer.zero_grad();
        loss.backward();

        optimizer.step();
        l = loss.data()[0];
    }
    assert_in_delta(f64::from(l), 0.0, 0.2);
}

#[test]
#[ignore = "needs the iris dataset from cdn.gorse.io (PORTING_NOTES.md O-1)"]
fn test_iris() {
    let (x, y) = load_iris(&dataset("iris").join("iris.data"));

    let model = new_sequential(vec![
        Box::new(new_linear(4, 100)),
        Box::new(new_linear(100, 100)),
        Box::new(new_linear(100, 3)),
    ]);
    let mut optimizer = new_adam(model.parameters(), 0.01);

    let mut l = 0.0;
    for _ in 0..1000 {
        let y_pred = model.forward(&x);
        let loss = softmax_cross_entropy(&y_pred, &y);

        optimizer.zero_grad();
        loss.backward();

        optimizer.step();
        l = loss.data()[0];
    }
    assert_in_delta(f64::from(l), 0.0, 0.1);
}

#[test]
#[ignore = "needs the mnist dataset from cdn.gorse.io (PORTING_NOTES.md O-1); skips unless Apple silicon or AVX-512, as Go does"]
fn test_mnist() {
    let path = dataset("mnist");
    let (train_a, train_b) = open_mnist_file(&path.join("train.libfm"));
    let (test_a, test_b) = open_mnist_file(&path.join("test.libfm"));

    let mut model = new_sequential(vec![
        Box::new(new_linear(784, 1000)),
        Box::new(new_relu()),
        Box::new(new_linear(1000, 10)),
    ]);
    model.set_jobs(std::thread::available_parallelism().map_or(1, |n| n.get()));
    let mut optimizer = new_adam(model.parameters(), 0.001);

    const BATCH_SIZE: usize = 1000;
    const NUM_EPOCH: usize = 5;
    let rows = train_a.shape()[0];
    for i in 0..NUM_EPOCH {
        let start_time = Instant::now();
        let (mut sum_loss, mut sum_acc) = (0.0f32, 0.0f32);
        for j in (0..rows).step_by(BATCH_SIZE) {
            let x_batch = train_a.slice(j, j + BATCH_SIZE);
            let y_batch = train_b.slice(j, j + BATCH_SIZE);

            let y_pred = model.forward(&x_batch);
            let loss = softmax_cross_entropy(&y_pred, &y_batch);

            optimizer.zero_grad();
            loss.backward();

            optimizer.step();
            sum_loss += loss.data()[0];
            sum_acc += accuracy(&y_pred, &y_batch);
        }
        sum_loss /= (rows / BATCH_SIZE) as f32;
        sum_acc /= (rows / BATCH_SIZE) as f32;
        println!(
            "Epoch {}/{}: Duration: {:?} Loss: {sum_loss} Accuracy: {sum_acc}",
            i + 1,
            NUM_EPOCH,
            start_time.elapsed()
        );
    }

    set_inference_mode(true);
    let test_acc = accuracy(&model.forward(&test_a), &test_b);
    set_inference_mode(false);
    println!("Test Accuracy: {test_acc}");
    assert!(f64::from(test_acc) > 0.95);
}

#[test]
fn test_save_and_load() {
    let (x, y) = spiral();

    let model = new_sequential(vec![
        Box::new(new_linear(2, 10)),
        Box::new(new_sigmoid()),
        Box::new(new_linear(10, 3)),
    ]);
    let mut optimizer = new_adam(model.parameters(), 0.01);

    let mut expected = 0.0;
    for _ in 0..300 {
        let y_pred = model.forward(&x);
        let loss = softmax_cross_entropy(&y_pred, &y);

        optimizer.zero_grad();
        loss.backward();

        optimizer.step();
        expected = loss.data()[0];
    }

    let mut buffer = Vec::new();
    save(&model, &mut buffer).expect("save");
    let model_loaded = new_sequential(vec![
        Box::new(new_linear(2, 10)),
        Box::new(new_sigmoid()),
        Box::new(new_linear(10, 3)),
    ]);
    load(&model_loaded, &mut buffer.as_slice()).expect("load");
    let y_pred = model_loaded.forward(&x);
    let loss = softmax_cross_entropy(&y_pred, &y);
    assert_in_delta(f64::from(expected), f64::from(loss.data()[0]), 0.01);
}
