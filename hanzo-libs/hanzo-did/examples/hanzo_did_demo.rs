//! Demo of W3C DID usage with omnichain identity support

use hanzo_did::{DID, DIDDocument, VerificationMethod, Service};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hanzo Omnichain DID Demo");
    println!("========================");

    // Simple mainnet DIDs (recommended format)
    let hanzo_did = DID::hanzo("zeekay");
    let lux_did = DID::lux("zeekay");
    println!("Mainnet DIDs:");
    println!("  Hanzo:  {}", hanzo_did);
    println!("  Lux:    {}", lux_did);

    // Local development DIDs
    let hanzo_local = DID::hanzo_local("zeekay");
    let lux_local = DID::lux_local("zeekay");
    println!("\nLocal Development:");
    println!("  Hanzo:  {}", hanzo_local);
    println!("  Lux:    {}", lux_local);

    // Native chain DIDs
    let eth_did = DID::eth("zeekay");
    let base_did = DID::base("zeekay");
    let polygon_did = DID::polygon("zeekay");
    println!("\nNative Chain DIDs:");
    println!("  Ethereum: {}", eth_did);
    println!("  Base:     {}", base_did);
    println!("  Polygon:  {}", polygon_did);

    // Context-aware resolution (@ format)
    let hanzo_context = DID::from_username("@zeekay", "hanzo");
    let lux_context = DID::from_username("@zeekay", "lux");
    println!("\nContext-Aware Resolution:");
    println!("  @zeekay in Hanzo app: {}", hanzo_context);
    println!("  @zeekay in Lux app:   {}", lux_context);

    // Omnichain identity verification
    println!("\nOmnichain Identity Verification:");
    println!("  hanzo:zeekay == lux:zeekay? {}", hanzo_did.is_same_entity(&lux_did));
    println!("  hanzo:zeekay == eth:zeekay? {}", hanzo_did.is_same_entity(&eth_did));
    println!("  eth:zeekay == base:zeekay? {}", eth_did.is_same_entity(&base_did));
    println!("  Base identifier: '{}'", hanzo_did.get_identifier());

    // Show all network variants for this identity
    println!("\nAll Network Variants for 'zeekay':");
    let variants = hanzo_did.get_omnichain_variants();
    for (i, variant) in variants.iter().enumerate() {
        println!("  {}: {}", i + 1, variant);
    }

    // Create a DID Document for the main identity
    let mut did_doc = DIDDocument::new(&hanzo_did);

    // Add verification method (example public key)
    let example_key = [
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
    ];

    let vm = VerificationMethod::new_ed25519(
        format!("{}#keys-1", hanzo_did),
        hanzo_did.to_string(),
        &example_key,
    );

    did_doc.verification_method = Some(vec![vm.clone()]);
    did_doc.authentication = Some(vec![vm.id.into()]);

    // Add service endpoints
    let hanzo_service = Service::hanzo_node(
        &hanzo_did.to_string(),
        "https://api.hanzo.network".to_string(),
    );

    let messaging_service = Service::messaging(
        &hanzo_did.to_string(),
        "wss://ws.hanzo.network".to_string(),
    );

    did_doc.service = Some(vec![hanzo_service, messaging_service]);

    println!("\nDID Document for {}:", hanzo_did);
    println!("{}", serde_json::to_string_pretty(&did_doc)?);

    println!("\n=== Omnichain Identity Authority ===");
    println!("✓ @zeekay in Hanzo app -> did:hanzo:zeekay (Hanzo mainnet)");
    println!("✓ @zeekay in Lux app   -> did:lux:zeekay (Lux mainnet)");
    println!("✓ Cross-chain verification: zeekay on any network = same entity");
    println!("✓ Independent identity authority spanning all supported networks");
    println!("✓ Context-aware resolution with omnichain verification capability");

    Ok(())
}