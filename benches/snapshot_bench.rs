use criterion::{criterion_group, criterion_main, Criterion};

#[derive(Clone, Copy)]
struct TestState {
	x: f32,
	y: f32,
	z: f32,
	health: u32,
	mana: u32,
	flags: [u8; 64],
}

fn bench_snapshot_delta(c: &mut Criterion) {
	c.bench_function("snapshot_delta_path", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<TestState>::with_defaults(128 * 1024).unwrap();
			let state = TestState {
				x: 1.0,
				y: 2.0,
				z: 3.0,
				health: 100,
				mana: 50,
				flags: [0u8; 64],
			};
			// First snapshot is always a full snapshot
			buf.snapshot(&state).unwrap();
			// Subsequent snapshots take the delta path
			for _ in 0..1000 {
				buf.snapshot(&state).unwrap();
			}
		});
	});
}

fn bench_snapshot_full(c: &mut Criterion) {
	c.bench_function("snapshot_full_path", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<TestState>::with_defaults(256 * 1024).unwrap();
			let mut state = TestState {
				x: 1.0,
				y: 2.0,
				z: 3.0,
				health: 100,
				mana: 50,
				flags: [0u8; 64],
			};
			// Force full snapshots by changing all bytes each time
			for i in 0..100 {
				state.x = i as f32;
				state.flags[i % 64] = i as u8;
				buf.snapshot(&state).unwrap();
			}
		});
	});
}

criterion_group!(benches, bench_snapshot_delta, bench_snapshot_full);
criterion_main!(benches);
