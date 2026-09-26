use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct DocState {
	cursor: u32,
	scroll_y: u32,
	selection_start: u32,
	selection_end: u32,
	dirty: bool,
	_pad: [u8; 3],
	line_hashes: [u64; 16],
}

fn make_doc_state(id: usize) -> DocState {
	DocState {
		cursor: (id * 7) as u32,
		scroll_y: (id / 4) as u32,
		selection_start: (id * 3) as u32,
		selection_end: (id * 3 + 5) as u32,
		dirty: id.is_multiple_of(3),
		_pad: [0; 3],
		line_hashes: std::array::from_fn(|i| {
			((id * 17 + i * 31) as u64).wrapping_mul(0x9E3779B97F4A7C15)
		}),
	}
}

fn bench_rollback(c: &mut Criterion) {
	let mut group = c.benchmark_group("rollback_undo");

	for depth in [1, 5, 10, 30, 60] {
		group.bench_with_input(BenchmarkId::new("undo_depth", depth), &depth, |b, &depth| {
			b.iter(|| {
				let mut buf = phoros::BPRB::<DocState>::with_defaults(128 * 1024).unwrap();
				// Simulate typing: each keystroke records a snapshot
				for i in 0..depth + 10 {
					buf.snapshot(&make_doc_state(i)).unwrap();
				}
				// Undo back to the beginning
				buf.rollback_to(0).unwrap();
			});
		});
	}

	group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
	let mut group = c.benchmark_group("snapshot_keystroke");

	// Simulate rapid typing: state barely changes between frames
	group.bench_function("typing_same_line", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<DocState>::with_defaults(128 * 1024).unwrap();
			let base = make_doc_state(0);
			for i in 0..1000 {
				let mut state = base;
				state.cursor += i as u32;
				buf.snapshot(&state).unwrap();
			}
		});
	});

	// Simulate edits across multiple lines: state changes more
	group.bench_function("editing_across_lines", |b| {
		b.iter(|| {
			let mut buf = phoros::BPRB::<DocState>::with_defaults(128 * 1024).unwrap();
			for i in 0..1000 {
				buf.snapshot(&make_doc_state(i)).unwrap();
			}
		});
	});

	group.finish();
}

criterion_group!(benches, bench_rollback, bench_snapshot);
criterion_main!(benches);
