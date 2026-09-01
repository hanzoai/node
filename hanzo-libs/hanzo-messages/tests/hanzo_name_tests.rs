#[cfg(test)]
mod tests {

    use hanzo_messages::hanzo_utils::signatures::unsafe_deterministic_signature_keypair;
    use hanzo_messages::schemas::hanzo_name::HanzoName;

    #[test]
    fn test_valid_names() {
        let valid_names = vec![
            "did:hanzo:alice",
            "did:hanzo:ALICE",
            "did:hanzo:alice_in_chains",
            "did:hanzo:alice/profileName",
            "did:hanzo:alice/profileName/agent/myChatGPTAgent",
            "did:hanzo:alice/profileName/device/myPhone",
            "did:hanzo:_my_9552/main",
            "did:hanzo:c85950e4d9c2c24df9d44052c8f2298f69d4a9b82cb0266478f58f385ca2a679/main",
        ];

        for name in valid_names {
            let result = HanzoName::new(name.to_string());
            assert!(result.is_ok(), "Expected {} to be valid, but it was not.", name);
        }
    }

    #[test]
    fn test_at_names_are_refused() {
        let refused = vec![
            "@@alice.hanzo",
            "@@alice.sep-hanzo",
            "@@alice.sep-hanzo/profileName",
            "@@alice.arb-sep-hanzo",
            "@@alice.sepolia-hanzo",
            "@@localhost.sep-hanzo",
            "@@localhost.sep-hanzo/main",
            "@@alice",
            "alice.hanzo",
            "alice",
            "did:lux:alice",
        ];

        for name in refused {
            assert!(
                HanzoName::new(name.to_string()).is_err(),
                "Expected {} to be refused, but it was accepted.",
                name
            );
            assert!(
                !HanzoName::is_fully_valid(name.to_string()),
                "Expected {} to be refused, but it was accepted.",
                name
            );
        }
    }

    #[test]
    fn test_invalid_names() {
        let invalid_names = vec![
            "did:hanzo:alice/profileName/myPhone",
            "did:hanzo:al!ce",
            "did:hanzo:alice-not-in-chains",
            "did:hanzo:alice//",
            "did:hanzo:alice//subidentity",
            "did:hanzo:alice/profile_1.hanzo",
            "did:hanzo:alice/profileName/agent",
            "did:hanzo:alice/profileName/agent/name/extra",
            "did:hanzo:",
            "did:hanzo",
        ];

        for name in invalid_names {
            let result = HanzoName::new(name.to_string());
            assert!(result.is_err(), "Expected {} to be invalid, but it was not.", name);
        }
    }

    #[test]
    fn test_from_node_and_profile_names_valid() {
        let result = HanzoName::from_node_and_profile_names("did:hanzo:bob".to_string(), "profileBob".to_string());
        assert!(result.is_ok(), "Expected the name to be valid");
    }

    #[test]
    fn test_from_node_and_profile_names_invalid() {
        let result = HanzoName::from_node_and_profile_names("b!ob".to_string(), "profileBob".to_string());
        assert!(result.is_err(), "Expected the name to be invalid");
    }

    #[test]
    fn test_has_profile() {
        let hanzo_name = HanzoName::new("did:hanzo:charlie/profileCharlie".to_string()).unwrap();
        assert!(hanzo_name.has_profile());
    }

    #[test]
    fn test_has_device() {
        let hanzo_name = HanzoName::new("did:hanzo:dave/profileDave/device/myDevice".to_string()).unwrap();
        assert!(hanzo_name.has_device());
    }

    #[test]
    fn test_has_no_subidentities() {
        let hanzo_name = HanzoName::new("did:hanzo:eve".to_string()).unwrap();
        assert!(!hanzo_name.has_profile(), "Name shouldn't have a profile");
        assert!(!hanzo_name.has_device(), "Name shouldn't have a device");
        assert!(hanzo_name.has_no_subidentities(), "Name should have no subidentities");
    }

    #[test]
    fn test_get_profile_name_string() {
        let hanzo_name = HanzoName::new("did:hanzo:frank/profileFrank".to_string()).unwrap();
        assert_eq!(hanzo_name.get_profile_name_string(), Some("profilefrank".to_string()));

        let hanzo_name = HanzoName::new("did:hanzo:frank/profile_1/device/device_1".to_string()).unwrap();
        assert_eq!(hanzo_name.get_profile_name_string(), Some("profile_1".to_string()));
    }

    #[test]
    fn test_extract_profile() {
        let hanzo_name = HanzoName::new("did:hanzo:frank/profileFrank".to_string()).unwrap();
        let extracted = hanzo_name.extract_profile();
        assert!(extracted.is_ok(), "Extraction should be successful");
        assert_eq!(extracted.unwrap().to_string(), "did:hanzo:frank/profilefrank");
    }

    #[test]
    fn test_extract_node() {
        let hanzo_name = HanzoName::new("did:hanzo:henry/profileHenry/device/myDevice".to_string()).unwrap();
        let node = hanzo_name.extract_node();
        assert_eq!(node.to_string(), "did:hanzo:henry");
    }

    #[test]
    fn test_contains() {
        let alice = HanzoName::new("did:hanzo:alice".to_string()).unwrap();
        let alice_profile = HanzoName::new("did:hanzo:alice/profileName".to_string()).unwrap();
        let alice_agent = HanzoName::new("did:hanzo:alice/profileName/agent/myChatGPTAgent".to_string()).unwrap();
        let alice_device = HanzoName::new("did:hanzo:alice/profileName/device/myDevice".to_string()).unwrap();

        assert!(alice.contains(&alice_profile));
        assert!(alice.contains(&alice_agent));
        assert!(alice_profile.contains(&alice_agent));
        assert!(alice_profile.contains(&alice_profile));
        assert!(alice_profile.contains(&alice_device));

        assert!(!alice_profile.contains(&alice));
        assert!(!alice_device.contains(&alice_profile));
    }

    #[test]
    fn test_does_not_contain() {
        let alice = HanzoName::new("did:hanzo:alice".to_string()).unwrap();
        let bob = HanzoName::new("did:hanzo:bob".to_string()).unwrap();
        let alice_profile = HanzoName::new("did:hanzo:alice/profileName".to_string()).unwrap();
        let alice_agent = HanzoName::new("did:hanzo:alice/profileName/agent/bobsGPT".to_string()).unwrap();
        let bob_agent = HanzoName::new("did:hanzo:bob/profileName/agent/myChatGPTAgent".to_string()).unwrap();

        assert!(!alice.contains(&bob));
        assert!(!bob.contains(&alice));
        assert!(!alice_profile.contains(&bob));
        assert!(!bob.contains(&alice_profile));
        assert!(!alice_agent.contains(&bob_agent));
    }

    #[test]
    fn test_get_fullname_string_without_node_name() {
        let hanzo_name1 = HanzoName::new("did:hanzo:alice".to_string()).unwrap();
        assert_eq!(hanzo_name1.get_fullname_string_without_node_name(), None);

        let hanzo_name2 = HanzoName::new("did:hanzo:alice/profileName".to_string()).unwrap();
        assert_eq!(
            hanzo_name2.get_fullname_string_without_node_name(),
            Some("profilename".to_string())
        );

        let hanzo_name3 = HanzoName::new("did:hanzo:alice/profileName/agent/myChatGPTAgent".to_string()).unwrap();
        assert_eq!(
            hanzo_name3.get_fullname_string_without_node_name(),
            Some("profilename/agent/mychatgptagent".to_string())
        );

        let hanzo_name4 = HanzoName::new("did:hanzo:alice/profileName/device/myPhone".to_string()).unwrap();
        assert_eq!(
            hanzo_name4.get_fullname_string_without_node_name(),
            Some("profilename/device/myphone".to_string())
        );
    }

    #[test]
    fn test_did_from_key() {
        let (_, public_key) = unsafe_deterministic_signature_keypair(0);
        let did = HanzoName::did(&public_key);

        // Pinned: the derivation names every fresh node and its database directory,
        // so a change here renames both.
        assert_eq!(
            did,
            "did:hanzo:c85950e4d9c2c24df9d44052c8f2298f69d4a9b82cb0266478f58f385ca2a679"
        );
        assert_eq!(HanzoName::new(did.clone()).unwrap().full_name, did);

        // The shapes the node builds on top of its own name.
        for suffix in ["/main", "/main/agent/myagent", "/main/device/myphone"] {
            let name = format!("{}{}", did, suffix);
            assert!(HanzoName::new(name.clone()).is_ok(), "expected {} to be valid", name);
        }
    }
}
