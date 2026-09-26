use criterion::{criterion_group, criterion_main, Criterion};

fn bench_encode_zero_delta(c: &mut Criterion) {
	c.bench_function("encode_zero_delta_256", |b| {
		let delta = vec![0u8; 256];
		let mut output = vec![0u8; 9 * 32]; // max encoded size
		b.iter(|| {
			phoros::codec::byte_masked_encode(&delta, &mut output)
		});
	});
}

fn bench_encode_sparse_delta(c: &mut Criterion) {
	c.bench_function("encode_sparse_delta_256", |b| {
		let mut delta = vec![0u8; 256];
		// Set 10% of bytes to non-zero
		for i in (0..256).step_by(10) {
			delta[i] = 0xAB;
		}
		let mut output = vec![0u8; 9 * 32];
		b.iter(|| {
			phoros::codec::byte_masked_encode(&delta, &mut output)
		});
	});
}

fn bench_encode_full_delta(c: &mut Criterion) {
	c.bench_function("encode_full_delta_256", |b| {
		let delta = vec![0xAB; 256];
		let mut output = vec![0u8; 9 * 32];
		b.iter(|| {
			phoros::codec::byte_masked_encode(&delta, &mut output)
		});
	});
}

fn bench_decode(c: &mut Criterion) {
	c.bench_function("decode_sparse_delta_256", |b| {
		let mut delta = vec![0u8; 256];
		for i in (0..256).step_by(10) {
			delta[i] = 0xAB;
		}
		let mut encoded = vec![0u8; 9 * 32];
		let len = phoros::codec::byte_masked_encode(&delta, &mut encoded);
		encoded.truncate(len);

		b.iter(|| {
			let mut output = vec![0u8; 256];
			phoros::codec::byte_masked_decode(&encoded, &mut output).unwrap();
		});
	});
}

criterion_group!(
	benches,
	bench_encode_zero_delta,
	bench_encode_sparse_delta,
	bench_encode_full_delta,
	bench_decode,
);
criterion_main!(benches);
