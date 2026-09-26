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

fn bench_rollback_shallow(c: &mut Criterion) {
	c.bench_function("rollback_1_frame", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<TestState>::with_defaults(64 * 1024).unwrap();
			let state = TestState {
				x: 1.0,
				y: 2.0,
				z: 3.0,
				health: 100,
				mana: 50,
				flags: [0u8; 64],
			};
			for _ in 0..10 {
				buf.snapshot(&state).unwrap();
			}
			buf.rollback_to(0).unwrap();
		});
	});
}

fn bench_rollback_deep(c: &mut Criterion) {
	c.bench_function("rollback_30_frames", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<TestState>::with_defaults(128 * 1024).unwrap();
			let mut state = TestState {
				x: 1.0,
				y: 2.0,
				z: 3.0,
				health: 100,
				mana: 50,
				flags: [0u8; 64],
			};
			for i in 0..60 {
				state.x += 0.1;
				state.health = 100 - i;
				buf.snapshot(&state).unwrap();
			}
			buf.rollback_to(29).unwrap();
		});
	});
}

criterion_group!(benches, bench_rollback_shallow, bench_rollback_deep);
criterion_main!(benches);
