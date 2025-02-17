use std::ops::Bound;

use common::{Index, IndexType};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dataflow_state::{
    PersistenceParameters, PersistentState, PointKey, RangeKey, SnapshotMode, State,
};
use itertools::Itertools;
use readyset_data::DfValue;

const UNIQUE_ENTIRES: usize = 100000;

lazy_static::lazy_static! {
    static ref STATE: PersistentState = {
        let mut state = PersistentState::new(
            String::from("bench"),
            vec![&[0usize][..], &[3][..]],
            &PersistenceParameters::default(),
        ).unwrap();

        state.add_key(Index::new(IndexType::HashMap, vec![0]), None);
        state.add_key(Index::new(IndexType::HashMap, vec![1, 2]), None);
        state.add_key(Index::new(IndexType::HashMap, vec![3]), None);

        state.add_key(Index::new(IndexType::BTreeMap, vec![1]), None);

        state.set_snapshot_mode(SnapshotMode::SnapshotModeEnabled);

        let animals = ["Cat", "Dog", "Bat"];

        for i in 0..UNIQUE_ENTIRES {
            let rec: Vec<DfValue> = vec![
                i.into(),
                animals[i % 3].into(),
                (i % 99).into(),
                i.into(),
            ];
            state.process_records(&mut vec![rec].into(), None, None).unwrap();
        }

        state.set_snapshot_mode(SnapshotMode::SnapshotModeDisabled);

        state
    };

    static ref LARGE_STRINGS: Vec<String> = ["a", "b", "c"].iter().map(|s| {
        std::iter::once(s).cycle().take(10000).join("")
    }).collect::<Vec<_>>();

    static ref STATE_LARGE_STRINGS: PersistentState = {
        let mut state = PersistentState::new(
            String::from("bench"),
            vec![&[0usize][..], &[3][..]],
            &PersistenceParameters::default(),
        ).unwrap();

        state.set_snapshot_mode(SnapshotMode::SnapshotModeEnabled);

        state.add_key(Index::new(IndexType::HashMap, vec![0]), None);
        state.add_key(Index::new(IndexType::HashMap, vec![1]), None);
        state.add_key(Index::new(IndexType::HashMap, vec![3]), None);

        state.add_key(Index::new(IndexType::BTreeMap, vec![1]), None);

        for i in 0..UNIQUE_ENTIRES {
            let rec: Vec<DfValue> = vec![
                i.into(),
                LARGE_STRINGS[i % 3].clone().into(),
                (i % 99).into(),
                i.into(),
            ];
            state.process_records(&mut vec![rec].into(), None, None).unwrap();
        }

        state.set_snapshot_mode(SnapshotMode::SnapshotModeDisabled);

        state
    };
}

pub fn rocksdb_get_primary_key(c: &mut Criterion) {
    let state = &*STATE;

    let mut group = c.benchmark_group("RockDB get primary key");
    group.bench_function("lookup_multi", |b| {
        let mut iter = 0usize;
        b.iter(|| {
            black_box(state.lookup_multi(
                &[0],
                &[
                    PointKey::Single(iter.into()),
                    PointKey::Single((iter + 100).into()),
                    PointKey::Single((iter + 200).into()),
                    PointKey::Single((iter + 300).into()),
                    PointKey::Single((iter + 400).into()),
                    PointKey::Single((iter + 500).into()),
                    PointKey::Single((iter + 600).into()),
                    PointKey::Single((iter + 700).into()),
                    PointKey::Single((iter + 800).into()),
                    PointKey::Single((iter + 900).into()),
                ],
            ));
            iter = (iter + 1) % (UNIQUE_ENTIRES - 1000);
        })
    });

    group.bench_function("lookup", |b| {
        let mut iter = 0usize;
        b.iter(|| {
            black_box((
                state.lookup(&[0], &PointKey::Single(iter.into())),
                state.lookup(&[0], &PointKey::Single((iter + 100).into())),
                state.lookup(&[0], &PointKey::Single((iter + 200).into())),
                state.lookup(&[0], &PointKey::Single((iter + 300).into())),
                state.lookup(&[0], &PointKey::Single((iter + 400).into())),
                state.lookup(&[0], &PointKey::Single((iter + 500).into())),
                state.lookup(&[0], &PointKey::Single((iter + 600).into())),
                state.lookup(&[0], &PointKey::Single((iter + 700).into())),
                state.lookup(&[0], &PointKey::Single((iter + 800).into())),
                state.lookup(&[0], &PointKey::Single((iter + 900).into())),
            ));
            iter = (iter + 1) % (UNIQUE_ENTIRES - 1000);
        })
    });

    group.finish();
}

pub fn rocksdb_get_secondary_key(c: &mut Criterion) {
    let state = &*STATE;

    let mut group = c.benchmark_group("RockDB get secondary key");
    group.bench_function("lookup_multi", |b| {
        b.iter(|| {
            black_box(state.lookup_multi(
                &[1, 2],
                &[
                    PointKey::Double(("Dog".into(), 1.into())),
                    PointKey::Double(("Cat".into(), 2.into())),
                ],
            ));
        })
    });

    group.bench_function("lookup", |b| {
        b.iter(|| {
            black_box((
                state.lookup(&[1, 2], &PointKey::Double(("Dog".into(), 1.into()))),
                state.lookup(&[1, 2], &PointKey::Double(("Cat".into(), 2.into()))),
            ))
        })
    });

    group.finish();
}

pub fn rocksdb_get_secondary_unique_key(c: &mut Criterion) {
    let state = &*STATE;

    let mut group = c.benchmark_group("RockDB get secondary unique key");
    group.bench_function("lookup_multi", |b| {
        let mut iter = 0usize;
        b.iter(|| {
            black_box(state.lookup_multi(
                &[3],
                &[
                    PointKey::Single(iter.into()),
                    PointKey::Single((iter + 100).into()),
                    PointKey::Single((iter + 200).into()),
                    PointKey::Single((iter + 300).into()),
                    PointKey::Single((iter + 400).into()),
                    PointKey::Single((iter + 500).into()),
                    PointKey::Single((iter + 600).into()),
                    PointKey::Single((iter + 700).into()),
                    PointKey::Single((iter + 800).into()),
                    PointKey::Single((iter + 900).into()),
                ],
            ));
            iter = (iter + 1) % (UNIQUE_ENTIRES - 1000);
        })
    });

    group.bench_function("lookup", |b| {
        let mut iter = 0usize;
        b.iter(|| {
            black_box((
                state.lookup(&[3], &PointKey::Single(iter.into())),
                state.lookup(&[3], &PointKey::Single((iter + 100).into())),
                state.lookup(&[3], &PointKey::Single((iter + 200).into())),
                state.lookup(&[3], &PointKey::Single((iter + 300).into())),
                state.lookup(&[3], &PointKey::Single((iter + 400).into())),
                state.lookup(&[3], &PointKey::Single((iter + 500).into())),
                state.lookup(&[3], &PointKey::Single((iter + 600).into())),
                state.lookup(&[3], &PointKey::Single((iter + 700).into())),
                state.lookup(&[3], &PointKey::Single((iter + 800).into())),
                state.lookup(&[3], &PointKey::Single((iter + 900).into())),
            ));
            iter = (iter + 1) % (UNIQUE_ENTIRES - 1000);
        })
    });

    group.finish();
}

pub fn rocksdb_range_lookup(c: &mut Criterion) {
    let state = &*STATE;
    let key = DfValue::from("D");

    let mut group = c.benchmark_group("RocksDB lookup_range");
    group.bench_function("lookup_range", |b| {
        b.iter(|| {
            black_box(state.lookup_range(
                &[1],
                &RangeKey::Single((Bound::Included(key.clone()), Bound::Unbounded)),
            ));
        })
    });
    group.finish();
}

pub fn rocksdb_range_lookup_large_strings(c: &mut Criterion) {
    let state = &*STATE_LARGE_STRINGS;
    let key = DfValue::from(LARGE_STRINGS[0].clone());

    let mut group = c.benchmark_group("RocksDB with large strings");
    group.bench_function("lookup_range", |b| {
        b.iter(|| {
            black_box(state.lookup_range(
                &[1],
                &RangeKey::Single((Bound::Included(key.clone()), Bound::Unbounded)),
            ));
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    rocksdb_get_primary_key,
    rocksdb_get_secondary_key,
    rocksdb_get_secondary_unique_key,
    rocksdb_range_lookup,
    rocksdb_range_lookup_large_strings,
);
criterion_main!(benches);
