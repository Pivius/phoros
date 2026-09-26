use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn bench_encode(c: &mut Criterion) {
	let mut group = c.benchmark_group("encode");

	for (name, size) in [("doc_state", 152), ("small_config", 32), ("large_state", 1024)] {
		group.bench_with_input(BenchmarkId::new("zero_delta", name), &size, |b, &size| {
			let delta = vec![0u8; size];
			let mut output = vec![0u8; 10 * size.div_ceil(8) + 1];
			b.iter(|| phoros::codec::byte_masked_encode(&delta, &mut output));
		});

		group.bench_with_input(BenchmarkId::new("sparse_10pct", name), &size, |b, &size| {
			let mut delta = vec![0u8; size];
			for i in (0..size).step_by(10) {
				delta[i] = 0xAB;
			}
			let mut output = vec![0u8; 10 * size.div_ceil(8) + 1];
			b.iter(|| phoros::codec::byte_masked_encode(&delta, &mut output));
		});

		group.bench_with_input(BenchmarkId::new("full_delta", name), &size, |b, &size| {
			let delta = vec![0xAB; size];
			let mut output = vec![0u8; 10 * size.div_ceil(8) + 1];
			b.iter(|| phoros::codec::byte_masked_encode(&delta, &mut output));
		});
	}

	group.finish();
}

fn bench_decode(c: &mut Criterion) {
	let mut group = c.benchmark_group("decode");

	for (name, size) in [("doc_state", 152), ("small_config", 32), ("large_state", 1024)] {
		let mut delta = vec![0u8; size];
		for i in (0..size).step_by(10) {
			delta[i] = 0xAB;
		}
		let mut encoded = vec![0u8; 10 * size.div_ceil(8) + 1];
		let len = phoros::codec::byte_masked_encode(&delta, &mut encoded);
		encoded.truncate(len);

		group.bench_with_input(BenchmarkId::new("sparse_10pct", name), &encoded, |b, enc| {
			b.iter(|| {
				let mut output = vec![0u8; size];
				phoros::codec::byte_masked_decode(enc, &mut output);
			});
		});
	}

	group.finish();
}

fn bench_crc16(c: &mut Criterion) {
	let mut group = c.benchmark_group("crc16");

	for (name, size) in [("doc_state", 152), ("small_config", 32), ("large_state", 1024)] {
		let data = vec![0x42u8; size];
		group.bench_with_input(BenchmarkId::new("compute", name), &data, |b, d| {
			b.iter(|| phoros::codec::crc16(d));
		});
	}

	group.finish();
}

criterion_group!(benches, bench_encode, bench_decode, bench_crc16);
criterion_main!(benches);
