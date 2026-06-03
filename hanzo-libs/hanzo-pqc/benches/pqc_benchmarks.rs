//! Benchmarks for PQC operations
//! 
//! Run with: cargo bench --package hanzo_pqc

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, MlKem},
    signature::{Signature, SignatureAlgorithm, MlDsa},
    hybrid::{HybridMode, HybridKem},
};
use tokio::runtime::Runtime;

fn kem_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let kem = MlKem::new();
    
    let mut group = c.benchmark_group("ML-KEM");
    
    for alg in [KemAlgorithm::MlKem512, KemAlgorithm::MlKem768, KemAlgorithm::MlKem1024] {
        // Benchmark key generation
        group.bench_with_input(
            BenchmarkId::new("keygen", format!("{:?}", alg)),
            &alg,
            |b, &alg| {
                b.to_async(&rt).iter(|| async {
                    let _keypair = kem.generate_keypair(alg).await.unwrap();
                });
            },
        );
        
        // Setup for encap/decap benchmarks
        let keypair = rt.block_on(kem.generate_keypair(alg)).unwrap();
        
        // Benchmark encapsulation
        group.bench_with_input(
            BenchmarkId::new("encapsulate", format!("{:?}", alg)),
            &keypair.encap_key,
            |b, encap_key| {
                b.to_async(&rt).iter(|| async {
                    let _output = kem.encapsulate(black_box(encap_key)).await.unwrap();
                });
            },
        );
        
        // Benchmark decapsulation
        let output = rt.block_on(kem.encapsulate(&keypair.encap_key)).unwrap();
        group.bench_with_input(
            BenchmarkId::new("decapsulate", format!("{:?}", alg)),
            &(keypair.decap_key, output.ciphertext),
            |b, (decap_key, ciphertext)| {
                b.to_async(&rt).iter(|| async {
                    let _secret = kem.decapsulate(
                        black_box(decap_key),
                        black_box(ciphertext),
                    ).await.unwrap();
                });
            },
        );
    }
    
    group.finish();
}

fn signature_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let dsa = MlDsa::new();
    
    let mut group = c.benchmark_group("ML-DSA");
    
    for alg in [SignatureAlgorithm::MlDsa44, SignatureAlgorithm::MlDsa65, SignatureAlgorithm::MlDsa87] {
        // Benchmark key generation
        group.bench_with_input(
            BenchmarkId::new("keygen", format!("{:?}", alg)),
            &alg,
            |b, &alg| {
                b.to_async(&rt).iter(|| async {
                    let _keypair = dsa.generate_keypair(alg).await.unwrap();
                });
            },
        );
        
        // Setup for sign/verify benchmarks
        let (verifying_key, signing_key) = rt.block_on(dsa.generate_keypair(alg)).unwrap();
        let message = b"Benchmark message for quantum-safe signatures";
        
        // Benchmark signing
        group.bench_with_input(
            BenchmarkId::new("sign", format!("{:?}", alg)),
            &(signing_key.clone(), message),
            |b, (signing_key, message)| {
                b.to_async(&rt).iter(|| async {
                    let _sig = dsa.sign(black_box(signing_key), black_box(*message)).await.unwrap();
                });
            },
        );
        
        // Benchmark verification
        let signature = rt.block_on(dsa.sign(&signing_key, message)).unwrap();
        group.bench_with_input(
            BenchmarkId::new("verify", format!("{:?}", alg)),
            &(verifying_key, message, signature),
            |b, (verifying_key, message, signature)| {
                b.to_async(&rt).iter(|| async {
                    let _valid = dsa.verify(
                        black_box(verifying_key),
                        black_box(*message),
                        black_box(signature),
                    ).await.unwrap();
                });
            },
        );
    }
    
    group.finish();
}

fn hybrid_benchmarks(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("Hybrid-KEM");
    
    for mode in [
        HybridMode::MlKem512X25519,
        HybridMode::MlKem768X25519,
        HybridMode::MlKem1024X25519,
    ] {
        let hybrid_kem = HybridKem::new(mode);
        
        // Benchmark key generation
        group.bench_with_input(
            BenchmarkId::new("keygen", format!("{:?}", mode)),
            &mode,
            |b, &mode| {
                b.to_async(&rt).iter(|| async {
                    let _keypair = hybrid_kem.generate_keypair(mode).await.unwrap();
                });
            },
        );
        
        // Setup for encap/decap benchmarks
        let (encap_key, decap_key) = rt.block_on(hybrid_kem.generate_keypair(mode)).unwrap();
        
        // Benchmark encapsulation
        group.bench_with_input(
            BenchmarkId::new("encapsulate", format!("{:?}", mode)),
            &encap_key,
            |b, encap_key| {
                b.to_async(&rt).iter(|| async {
                    let _output = hybrid_kem.encapsulate(black_box(encap_key)).await.unwrap();
                });
            },
        );
        
        // Benchmark decapsulation
        let output = rt.block_on(hybrid_kem.encapsulate(&encap_key)).unwrap();
        group.bench_with_input(
            BenchmarkId::new("decapsulate", format!("{:?}", mode)),
            &(decap_key, output.ciphertext),
            |b, (decap_key, ciphertext)| {
                b.to_async(&rt).iter(|| async {
                    let _secret = hybrid_kem.decapsulate(
                        black_box(decap_key),
                        black_box(&ciphertext.pq_ciphertext),
                        black_box(&ciphertext.classical_ciphertext),
                    ).await.unwrap();
                });
            },
        );
    }
    
    group.finish();
}

fn kdf_benchmarks(c: &mut Criterion) {
    use hanzo_pqc::kdf::{Kdf, HkdfKdf, KdfAlgorithm};
    
    let mut group = c.benchmark_group("KDF");
    
    for alg in [KdfAlgorithm::HkdfSha256, KdfAlgorithm::HkdfSha384, KdfAlgorithm::HkdfSha512] {
        let kdf = HkdfKdf::new(alg);
        let ikm = b"input key material for KDF benchmark";
        let salt = Some(b"salt value".as_ref());
        let info = b"hanzo-pqc-benchmark";
        
        group.bench_with_input(
            BenchmarkId::new("derive_32", format!("{:?}", alg)),
            &(ikm, salt, info),
            |b, &(ikm, salt, info)| {
                b.iter(|| {
                    let _key = kdf.derive(salt, ikm, info, 32).unwrap();
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("derive_64", format!("{:?}", alg)),
            &(ikm, salt, info),
            |b, &(ikm, salt, info)| {
                b.iter(|| {
                    let _key = kdf.derive(salt, ikm, info, 64).unwrap();
                });
            },
        );
    }
    
    // Benchmark combining shared secrets (for hybrid mode)
    use hanzo_pqc::kdf::combine_shared_secrets;
    
    let kdf = HkdfKdf::new(KdfAlgorithm::HkdfSha384);
    let secret1 = vec![0u8; 32]; // ML-KEM shared secret
    let secret2 = vec![0u8; 32]; // X25519 shared secret
    
    group.bench_function("combine_secrets", |b| {
        b.iter(|| {
            let _combined = combine_shared_secrets(
                &kdf,
                &[black_box(&secret1), black_box(&secret2)],
                b"hybrid-benchmark",
                32,
            ).unwrap();
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    kem_benchmarks,
    signature_benchmarks,
    hybrid_benchmarks,
    kdf_benchmarks
);
criterion_main!(benches);