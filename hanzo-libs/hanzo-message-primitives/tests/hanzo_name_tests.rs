#[cfg(test)]
mod tests {

    use hanzo_message_primitives::schemas::hanzo_name::HanzoName;

    #[test]
    fn test_valid_names() {
        println!("Testing valid names");
        let valid_names = vec![
            "@@alice.hanzo",
            "@@ALICE.HANZO",
            "@@alice_in_chains.hanzo",
            "@@alice/subidentity",
            "@@alice.hanzo/profileName",
            "@@alice.hanzo/profileName/agent/myChatGPTAgent",
            "@@alice.hanzo/profileName/device/myPhone",
            "@@alice.sep-hanzo",
            "@@alice.sep-hanzo/profileName",
            "@@alice.sep-hanzo/profileName/agent/myChatGPTAgent",
            "@@alice.sep-hanzo/profileName/device/myPhone",
            "@@_my_9552.sep-hanzo/main",
        ];

        for name in valid_names {
            let result = HanzoName::new(name.to_string());
            assert!(result.is_ok(), "Expected {} to be valid, but it was not.", name);
        }
    }

    #[test]
    fn test_invalid_names_with_repair() {
        let invalid_names = vec![
            "@@alice.hanzo/profileName/myPhone",
            "@@alice-not-in-chains.hanzo",
            "@alice.hanzo",
            "@@@alice.hanzo",
            "@@al!ce.hanzo",
            "@@alice.hanzo//",
            "@@alice.hanzo//subidentity",
            "@@node1.hanzo/profile_1.hanzo",
        ];

        for name in invalid_names {
            let result = HanzoName::new(name.to_string());
            assert!(result.is_err(), "Expected {} to be invalid, but it was not.", name);
        }
    }

    #[test]
    fn test_invalid_names_without_repair() {
        let invalid_names = vec![
            "@@alice.hanzo/profileName/myPhone",
            "@@al!ce.hanzo",
            "@@alice/subidentity",
            "@@alice.hanzo//",
            "@@alice.hanzo//subidentity",
            "@@node1.hanzo/profile_1.hanzo",
        ];

        for name in invalid_names {
            let result = HanzoName::is_fully_valid(name.to_string());
            assert!(!result, "Expected {} to be invalid, but it was not.", name);
        }
    }

    #[test]
    fn test_no_hanzo_suffix() {
        let name = "@@alice";
        let result = HanzoName::new(name.to_string());
        assert!(result.is_ok(), "Expected the name to be formatted correctly");
        assert_eq!(result.unwrap().to_string(), "@@alice.hanzo");
    }

    #[test]
    fn test_no_hanzo_prefix() {
        let name = "alice.hanzo";
        let result = HanzoName::new(name.to_string());
        assert!(result.is_ok(), "Expected the name to be formatted correctly");
        assert_eq!(result.unwrap().to_string(), "@@alice.hanzo");
    }

    #[test]
    fn test_from_node_and_profile_names_valid() {
        // Since the function can correct this, we just check for a valid response.
        let result = HanzoName::from_node_and_profile_names("bob.hanzo".to_string(), "profileBob".to_string());
        assert!(result.is_ok(), "Expected the name to be valid");
    }

    #[test]
    fn test_from_node_and_profile_names_invalid() {
        // If we want to ensure that the format isn't automatically fixed, we could use a clearly invalid name.
        let result = HanzoName::from_node_and_profile_names("b!ob".to_string(), "profileBob".to_string());
        assert!(result.is_err(), "Expected the name to be invalid");
    }

    #[test]
    fn test_has_profile() {
        let hanzo_name = HanzoName::new("@@charlie.hanzo/profileCharlie".to_string()).unwrap();
        assert!(hanzo_name.has_profile());
    }

    #[test]
    fn test_has_device() {
        let hanzo_name = HanzoName::new("@@dave.hanzo/profileDave/device/myDevice".to_string()).unwrap();
        assert!(hanzo_name.has_device());
    }

    #[test]
    fn test_has_no_subidentities() {
        let hanzo_name = HanzoName::new("@@eve.hanzo".to_string()).unwrap();
        assert!(!hanzo_name.has_profile(), "Name shouldn't have a profile");
        assert!(!hanzo_name.has_device(), "Name shouldn't have a device");
        assert!(hanzo_name.has_no_subidentities(), "Name should have no subidentities");
    }

    #[test]
    fn test_get_profile_name_string() {
        let hanzo_name = HanzoName::new("@@frank.hanzo/profileFrank".to_string()).unwrap();
        assert_eq!(hanzo_name.get_profile_name_string(), Some("profilefrank".to_string()));

        let hanzo_name = HanzoName::new("@@frank.hanzo/profile_1/device/device_1".to_string()).unwrap();
        assert_eq!(hanzo_name.get_profile_name_string(), Some("profile_1".to_string()));
    }

    #[test]
    fn test_extract_profile() {
        let hanzo_name = HanzoName::new("@@frank.hanzo/profileFrank".to_string()).unwrap();
        let extracted = hanzo_name.extract_profile();
        assert!(extracted.is_ok(), "Extraction should be successful");
        assert_eq!(extracted.unwrap().to_string(), "@@frank.hanzo/profilefrank");
    }

    #[test]
    fn test_extract_node() {
        let hanzo_name = HanzoName::new("@@henry.hanzo/profileHenry/device/myDevice".to_string()).unwrap();
        let node = hanzo_name.extract_node();
        assert_eq!(node.to_string(), "@@henry.hanzo");
    }

    #[test]
    fn test_contains() {
        let alice = HanzoName::new("@@alice.hanzo".to_string()).unwrap();
        let alice_profile = HanzoName::new("@@alice.hanzo/profileName".to_string()).unwrap();
        let alice_agent = HanzoName::new("@@alice.hanzo/profileName/agent/myChatGPTAgent".to_string()).unwrap();
        let alice_device = HanzoName::new("@@alice.hanzo/profileName/device/myDevice".to_string()).unwrap();

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
        let alice = HanzoName::new("@@alice.hanzo".to_string()).unwrap();
        let bob = HanzoName::new("@@bob.hanzo".to_string()).unwrap();
        let alice_profile = HanzoName::new("@@alice.hanzo/profileName".to_string()).unwrap();
        let alice_agent = HanzoName::new("@@alice.hanzo/profileName/agent/bobsGPT".to_string()).unwrap();
        let bob_agent = HanzoName::new("@@bob.hanzo/profileName/agent/myChatGPTAgent".to_string()).unwrap();

        assert!(!alice.contains(&bob));
        assert!(!bob.contains(&alice));
        assert!(!alice_profile.contains(&bob));
        assert!(!bob.contains(&alice_profile));
        assert!(!alice_agent.contains(&bob_agent));
    }

    #[test]
    fn test_get_fullname_string_without_node_name() {
        let hanzo_name1 = HanzoName::new("@@alice.hanzo".to_string()).unwrap();
        assert_eq!(hanzo_name1.get_fullname_string_without_node_name(), None);

        let hanzo_name2 = HanzoName::new("@@alice.hanzo/profileName".to_string()).unwrap();
        assert_eq!(
            hanzo_name2.get_fullname_string_without_node_name(),
            Some("profilename".to_string())
        );

        let hanzo_name3 = HanzoName::new("@@alice.hanzo/profileName/agent/myChatGPTAgent".to_string()).unwrap();
        assert_eq!(
            hanzo_name3.get_fullname_string_without_node_name(),
            Some("profilename/agent/mychatgptagent".to_string())
        );

        let hanzo_name4 = HanzoName::new("@@alice.hanzo/profileName/device/myPhone".to_string()).unwrap();
        assert_eq!(
            hanzo_name4.get_fullname_string_without_node_name(),
            Some("profilename/device/myphone".to_string())
        );
    }
}
