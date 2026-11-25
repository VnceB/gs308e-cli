#[path = "support/write_lab.rs"]
mod lab;
use lab::*;

fn configured_vlan() -> Lab {
    let mut lab = Lab::isolated();
    lab.current.shared.mode = Mode::AdvancedQ;
    lab.current.shared.vlans.insert(
        27,
        Vlan {
            name: "Audio".into(),
            members: [Membership::Excluded; 8],
        },
    );
    lab
}

#[test]
fn label_preview_preserves_port_edit_siblings_and_other_ports() {
    let mut lab = Lab::isolated();
    lab.current.ports[6] = PortSettings {
        label: "Desk".into(),
        speed: Speed::Full100,
        flow: false,
        ingress_kbps: Some(512),
        egress_kbps: Some(1024),
        priority: Priority::Critical,
    };
    lab.connections[0] = Connection {
        role: Role::Uplink,
        link: Link::Up,
    };
    let original = lab.current.clone();
    let plan = lab
        .preview(|s| s.ports[6].label = "Lab & bench".into())
        .unwrap();
    assert_eq!(lab.current, original);
    assert_eq!(lab.submissions, 0);
    let mut expected = original.clone();
    expected.ports[6].label = "Lab & bench".into();
    assert_eq!(*plan.after(), expected);
    lab.submit(&plan, Delivery::Ack).unwrap();
    assert_eq!(lab.verify(&plan, true), Readback::Applied);
    lab.restore(&plan, true).unwrap();
    assert_eq!(lab.current, original);
}

#[test]
fn rejects_invalid_port_numbers_labels_and_rates() {
    let lab = Lab::isolated();
    assert!(index(0).is_err() && index(9).is_err());
    assert_eq!((index(1), index(8)), (Ok(0), Ok(7)));
    for label in ["", "abcdefghijklmnopq", "a\nb", "a\u{7f}b", "pört"] {
        assert!(lab.preview(|c| c.ports[1].label = label.into()).is_err());
    }
    assert!(
        lab.preview(|c| c.ports[1].label = "abcdefghijklmnop".into())
            .is_ok()
    );
    assert!(
        lab.preview(|c| c.ports[1].ingress_kbps = Some(777))
            .is_err()
    );
    assert!(lab.preview(|c| c.ports[1].egress_kbps = Some(0)).is_err());
    assert_eq!(lab.submissions, 0);
}

#[test]
fn isolated_port_speed_priority_limits_and_flow_round_trip() {
    for speed in [Speed::Auto, Speed::Full10, Speed::Full100, Speed::Disabled] {
        for priority in [
            Priority::Low,
            Priority::Medium,
            Priority::High,
            Priority::Critical,
        ] {
            let mut lab = Lab::isolated();
            let plan = lab
                .preview(|c| {
                    let p = &mut c.ports[4];
                    p.speed = speed;
                    p.flow = false;
                    p.ingress_kbps = Some(512);
                    p.egress_kbps = Some(1024);
                    p.priority = priority;
                })
                .unwrap();
            lab.submit(&plan, Delivery::Ack).unwrap();
            assert_eq!(lab.current.ports[4], plan.after().ports[4]);
            lab.restore(&plan, true).unwrap();
            assert_eq!(lab.current, *plan.before());
        }
    }
}

#[test]
fn rejects_up_unknown_uplink_management_and_other_device_even_in_batch() {
    for (role, link) in [
        (Role::IsolatedFixture, Link::Up),
        (Role::Unknown, Link::Down),
        (Role::Uplink, Link::Down),
        (Role::ManagementComputer, Link::Down),
        (Role::OtherDevice, Link::Down),
        (Role::IsolatedFixture, Link::Unknown),
    ] {
        let mut lab = Lab::isolated();
        lab.connections[2] = Connection { role, link };
        assert_eq!(
            lab.preview(|c| {
                c.ports[5].label = "Fixture B".into();
                c.ports[2].speed = Speed::Disabled;
            })
            .unwrap_err(),
            "protected or unverified scope"
        );
        assert_eq!(lab.current, Config::default());
        assert_eq!(lab.submissions, 0);
    }
}

#[test]
fn checks_live_status_and_independent_role_again_before_apply_and_restore() {
    let mut lab = Lab::isolated();
    let plan = lab.preview(|s| s.ports[3].flow = false).unwrap();
    lab.connections[3].link = Link::Up;
    assert!(lab.submit(&plan, Delivery::Ack).is_err());
    assert_eq!(lab.submissions, 0);
    lab.connections[3].link = Link::Down;
    lab.submit(&plan, Delivery::Ack).unwrap();
    lab.connections[3].role = Role::ManagementComputer;
    assert!(lab.restore(&plan, true).is_err());
    assert!(!lab.current.ports[3].flow);
    assert_eq!(lab.submissions, 1);
}

#[test]
fn stale_preimage_and_no_op_never_submit() {
    let mut lab = Lab::isolated();
    let no_op = lab.preview(|_| {}).unwrap();
    lab.submit(&no_op, Delivery::Ack).unwrap();
    lab.restore(&no_op, true).unwrap();
    let plan = lab
        .preview(|s| s.ports[2].label = "Inventory".into())
        .unwrap();
    lab.current.ports[6].flow = false;
    assert_eq!(lab.submit(&plan, Delivery::Ack), Err("stale preimage"));
    assert_eq!(lab.submissions, 0);
    assert!(!lab.current.ports[6].flow);
}

#[test]
fn reconciles_lost_acknowledgements_and_checks_success_against_readback() {
    for (delivery, ok, expected) in [
        (Delivery::Rejected, false, Readback::Original),
        (Delivery::LostBeforeApply, false, Readback::Original),
        (Delivery::LostAfterApply, false, Readback::Applied),
        (Delivery::AckWithoutApply, true, Readback::Original),
    ] {
        let mut lab = Lab::isolated();
        let plan = lab.preview(|c| c.ports[0].speed = Speed::Full10).unwrap();
        assert_eq!(lab.submit(&plan, delivery).is_ok(), ok);
        assert_eq!(lab.verify(&plan, true), expected);
        assert_eq!(lab.submissions, 1);
        lab.restore(&plan, true).unwrap();
        assert_eq!(lab.current, *plan.before());
        assert_eq!(
            lab.submissions,
            if expected == Readback::Applied { 2 } else { 1 }
        );
    }
}

#[test]
fn refuses_blind_restore_after_read_failure_or_third_party_edit() {
    let mut lab = Lab::isolated();
    let p = lab.preview(|c| c.ports[3].speed = Speed::Full10).unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert_eq!(lab.verify(&p, false), Readback::Unavailable);
    assert!(lab.restore(&p, false).is_err());
    lab.current.ports[0].label = "Concurrent edit".into();
    let current = lab.current.clone();
    assert_eq!(lab.verify(&p, true), Readback::Diverged);
    assert!(lab.restore(&p, true).is_err());
    assert_eq!(lab.current, current);
    assert_eq!(lab.submissions, 1);
}

#[test]
fn device_wide_controls_require_an_entirely_isolated_fixture() {
    for toggle in [
        Toggle::Leds,
        Toggle::EnergySaving,
        Toggle::LoopPrevention,
        Toggle::BroadcastFiltering,
        Toggle::IgmpSnooping,
        Toggle::BlockUnknownMulticast,
        Toggle::ValidateIgmpHeader,
        Toggle::Discovery,
    ] {
        let mut lab = Lab::isolated();
        let edit = |s: &mut Config| {
            s.shared.toggles.insert(toggle);
        };
        lab.connections[7].role = Role::Unknown;
        assert!(lab.preview(edit).is_err());
        lab.connections[7].role = Role::IsolatedFixture;
        let plan = lab.preview(edit).unwrap();
        lab.submit(&plan, Delivery::Ack).unwrap();
        assert!(lab.current.shared.toggles.contains(&toggle));
        lab.restore(&plan, true).unwrap();
        assert_eq!(lab.current, *plan.before());
    }
}

#[test]
fn name_limit_and_qos_mode_affect_the_mock_form() {
    let mut lab = Lab::isolated();
    assert!(lab.preview(|c| c.shared.name = "x".repeat(21)).is_err());
    let p = lab
        .preview(|c| {
            c.shared.name = "x".repeat(20);
            c.shared.dscp = true;
        })
        .unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert!(
        lab.preview(|c| c.ports[0].priority = Priority::Critical)
            .is_err()
    );
    assert!(lab.preview(|c| c.ports[0].ingress_kbps = Some(512)).is_ok());
    lab.restore(&p, true).unwrap();
    assert_eq!(lab.current, *p.before());
}

#[test]
fn advanced_vlan_membership_and_pvid_require_a_consistent_table() {
    let mut lab = configured_vlan();
    assert!(lab.preview(|c| c.shared.pvid[3] = 27).is_err());
    let change = lab
        .preview(|c| {
            let v = c.shared.vlans.get_mut(&27).unwrap();
            v.members[3] = Membership::Untagged;
            v.members[5] = Membership::Tagged;
            c.shared.pvid[3] = 27;
        })
        .unwrap();
    lab.submit(&change, Delivery::Ack).unwrap();
    assert_eq!(lab.current.shared.pvid[3], 27);
    assert!(
        lab.preview(|c| {
            c.shared.vlans.remove(&27);
        })
        .is_err()
    );
    assert!(lab.preview(|c| c.shared.mode = Mode::Disabled).is_err());
    lab.restore(&change, true).unwrap();
    let deletion = lab
        .preview(|c| {
            c.shared.vlans.remove(&27);
        })
        .unwrap();
    lab.submit(&deletion, Delivery::Ack).unwrap();
    assert!(!lab.current.shared.vlans.contains_key(&27));
    lab.restore(&deletion, true).unwrap();
    assert_eq!(lab.current, *change.before());
}

#[test]
fn advanced_vlan_checks_mode_reserved_ids_capacity_and_name_length() {
    let mut lab = Lab::isolated();
    let add = |c: &mut Config, id, name: &str| {
        c.shared.vlans.insert(
            id,
            Vlan {
                name: name.into(),
                members: [Membership::Excluded; 8],
            },
        );
    };
    assert!(lab.preview(|c| add(c, 2, "Office")).is_err());
    let mode = lab.preview(|c| c.shared.mode = Mode::AdvancedQ).unwrap();
    lab.submit(&mode, Delivery::Ack).unwrap();
    for id in [0, 4094, 4095] {
        assert!(lab.preview(|c| add(c, id, "Office")).is_err());
    }
    assert!(
        lab.preview(|c| {
            c.shared.vlans.remove(&1);
        })
        .is_err()
    );
    assert!(lab.preview(|c| add(c, 4093, "12345678901234")).is_ok());
    assert!(lab.preview(|c| add(c, 2, "123456789012345")).is_err());
    assert!(
        lab.preview(|c| {
            for id in 2..=65 {
                add(c, id, "VLAN");
            }
        })
        .is_err()
    );
    lab.restore(&mode, true).unwrap();
}

#[test]
fn static_lags_check_membership_size_speed_flow_and_vlan_consistency() {
    let mut lab = configured_vlan();
    for invalid in [vec![1], vec![1, 2, 3, 4, 5], vec![2, 2], vec![8, 9]] {
        assert!(lab.preview(|c| c.shared.lags[0] = invalid).is_err());
    }
    let plan = lab
        .preview(|c| c.shared.lags = [vec![2, 4], vec![5, 6]])
        .unwrap();
    lab.submit(&plan, Delivery::Ack).unwrap();
    assert!(lab.preview(|c| c.shared.lags[1] = vec![4, 7]).is_err());
    assert!(lab.preview(|c| c.ports[1].speed = Speed::Full10).is_err());
    assert!(lab.preview(|c| c.ports[1].flow = false).is_err());
    assert!(
        lab.preview(|c| c.shared.vlans.get_mut(&27).unwrap().members[1] = Membership::Tagged)
            .is_err()
    );
    lab.restore(&plan, true).unwrap();
    assert_eq!(lab.current, *plan.before());
}

#[test]
fn mirror_sources_and_destination_are_checked_and_restored() {
    let mut lab = Lab::isolated();
    let mirror = |sources, destination| {
        Some(Mirror {
            sources,
            destination,
        })
    };
    for invalid in [
        mirror(vec![], 5),
        mirror(vec![3, 5], 5),
        mirror(vec![2, 2], 5),
        mirror(vec![0], 2),
        mirror(vec![1], 9),
    ] {
        assert!(lab.preview(|c| c.shared.mirror = invalid).is_err());
    }
    let p = lab
        .preview(|c| c.shared.mirror = mirror(vec![3, 4], 6))
        .unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert_eq!(lab.current.shared.mirror, mirror(vec![3, 4], 6));
    lab.connections[2].link = Link::Up;
    assert!(
        lab.preview(|c| c.shared.mirror = mirror(vec![2], 8))
            .is_err()
    );
    assert!(lab.restore(&p, true).is_err());
    lab.connections[2].link = Link::Down;
    lab.restore(&p, true).unwrap();
    assert_eq!(lab.current, *p.before());
}

#[test]
fn a_port_referenced_by_shared_configuration_inherits_its_scope() {
    for variant in [
        "lag",
        "mirror source",
        "mirror destination",
        "router port",
        "vlan",
    ] {
        let mut lab = configured_vlan();
        match variant {
            "router port" => lab.current.shared.router_port = Some(3),
            "lag" => lab.current.shared.lags[0] = vec![3, 4],
            "mirror source" => {
                lab.current.shared.mirror = Some(Mirror {
                    sources: vec![3],
                    destination: 4,
                })
            }
            "mirror destination" => {
                lab.current.shared.mirror = Some(Mirror {
                    sources: vec![4],
                    destination: 3,
                })
            }
            _ => lab.current.shared.vlans.get_mut(&27).unwrap().members[2] = Membership::Untagged,
        }
        lab.connections[7].role = Role::Uplink;
        assert!(
            lab.preview(|c| c.ports[2].label = "New label".into())
                .is_err()
        );
        assert_eq!(lab.submissions, 0);
    }
}

#[test]
fn basic_and_advanced_port_vlans_enforce_their_different_membership_rules() {
    for mode in [Mode::BasicPort, Mode::AdvancedPort] {
        let mut lab = Lab::isolated();
        lab.current.shared.mode = mode;
        let plan = lab
            .preview(|c| {
                c.shared.vlans.get_mut(&1).unwrap().members[2] = Membership::Excluded;
                let mut v = Vlan {
                    name: "Group".into(),
                    members: [Membership::Excluded; 8],
                };
                v.members[2] = Membership::Untagged;
                c.shared.vlans.insert(8, v);
            })
            .unwrap();
        lab.submit(&plan, Delivery::Ack).unwrap();
        assert_eq!(
            lab.preview(|c| c.shared.vlans.get_mut(&1).unwrap().members[2] = Membership::Untagged)
                .is_ok(),
            lab.current.shared.mode == Mode::AdvancedPort
        );
        assert!(
            lab.preview(|c| c.shared.vlans.get_mut(&8).unwrap().members[2] = Membership::Tagged)
                .is_err()
        );
        assert!(
            lab.preview(|c| {
                let v = c.shared.vlans.remove(&8).unwrap();
                c.shared.vlans.insert(9, v);
            })
            .is_err()
        );
        lab.restore(&plan, true).unwrap();
    }
}

#[test]
fn basic_802_1q_trunks_cover_every_vlan_and_access_ports_have_one_membership() {
    let mut lab = Lab::isolated();
    lab.current.shared.mode = Mode::BasicQ;
    let plan = lab
        .preview(|c| {
            c.shared.vlans.get_mut(&1).unwrap().members[3] = Membership::Tagged;
            let mut v = Vlan {
                name: "Video".into(),
                members: [Membership::Excluded; 8],
            };
            v.members[3] = Membership::Tagged;
            c.shared.vlans.insert(230, v);
        })
        .unwrap();
    lab.submit(&plan, Delivery::Ack).unwrap();
    assert!(
        lab.preview(|c| c.shared.vlans.get_mut(&230).unwrap().members[3] = Membership::Excluded)
            .is_err()
    );
    assert!(
        lab.preview(|c| c.shared.vlans.get_mut(&230).unwrap().members[4] = Membership::Untagged)
            .is_err()
    );
    lab.restore(&plan, true).unwrap();
}

#[test]
fn voice_vlan_cos_and_oui_table_are_bounded_and_restorable() {
    let mut lab = configured_vlan();
    let p = lab
        .preview(|c| {
            c.shared.voice = Some(Voice {
                vlan: 27,
                cos: 6,
                ouis: [[2, 0, 1]].into(),
            })
        })
        .unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert!(
        lab.preview(|c| c.shared.voice.as_mut().unwrap().cos = 8)
            .is_err()
    );
    assert!(
        lab.preview(|c| c.shared.voice.as_mut().unwrap().vlan = 99)
            .is_err()
    );
    assert!(
        lab.preview(
            |c| c.shared.voice.as_mut().unwrap().ouis = (0..16).map(|n| [2, 1, n]).collect()
        )
        .is_err()
    );
    assert!(lab.preview(|c| c.shared.mode = Mode::Disabled).is_err());
    lab.restore(&p, true).unwrap();
}

#[test]
fn multicast_checks_vlan_dependency_and_router_port() {
    let mut lab = Lab::isolated();
    assert!(lab.preview(|c| c.shared.igmp_vlan = Some(1)).is_err());
    lab = configured_vlan();
    let p = lab
        .preview(|c| {
            c.shared.igmp_vlan = Some(27);
            c.shared.router_port = Some(3);
        })
        .unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert!(lab.preview(|c| c.shared.router_port = Some(9)).is_err());
    assert!(
        lab.preview(|c| {
            c.shared.vlans.remove(&27);
        })
        .is_err()
    );
    lab.restore(&p, true).unwrap();
}

#[test]
fn partial_compound_change_requires_reconciliation_and_never_blindly_restores() {
    let mut lab = Lab::isolated();
    let p = lab
        .preview(|c| {
            c.ports[1].label = "Bench".into();
            c.ports[1].flow = false;
        })
        .unwrap();
    lab.current.ports[1].label = "Bench".into();
    assert_eq!(lab.verify(&p, true), Readback::Diverged);
    assert!(lab.restore(&p, true).is_err());
    assert_eq!(lab.submissions, 0);
    assert!(lab.current.ports[1].flow);
}

#[test]
fn static_ipv4_and_dhcp_are_inert_fixture_states_with_device_wide_scope() {
    use std::net::Ipv4Addr;
    let address = StaticIpv4 {
        address: Ipv4Addr::new(192, 0, 2, 20),
        prefix: 24,
        gateway: Ipv4Addr::new(192, 0, 2, 1),
    };
    let mut lab = Lab::isolated();
    lab.connections[4].role = Role::ManagementComputer;
    assert!(
        lab.preview(|c| c.shared.static_ipv4 = Some(address))
            .is_err()
    );
    lab.connections[4].role = Role::IsolatedFixture;
    for bad in [
        StaticIpv4 {
            gateway: address.address,
            ..address
        },
        StaticIpv4 {
            prefix: 33,
            ..address
        },
        StaticIpv4 {
            address: Ipv4Addr::new(192, 0, 2, 255),
            ..address
        },
        StaticIpv4 {
            gateway: Ipv4Addr::new(192, 0, 2, 0),
            ..address
        },
        StaticIpv4 {
            gateway: Ipv4Addr::new(198, 51, 100, 1),
            ..address
        },
    ] {
        assert!(lab.preview(|c| c.shared.static_ipv4 = Some(bad)).is_err());
    }
    let p = lab
        .preview(|c| c.shared.static_ipv4 = Some(address))
        .unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert_eq!(lab.current.shared.static_ipv4, Some(address));
    lab.restore(&p, true).unwrap();
    assert_eq!(lab.current.shared.static_ipv4, None);
}

#[test]
fn validates_documented_sntp_client_ranges_without_querying_a_time_server() {
    let mut lab = Lab::isolated();
    let settings = SntpClient {
        local_port: 123,
        interval_exponent: 6,
        timeout_seconds: 5,
        retries: 2,
    };
    for bad in [
        SntpClient {
            local_port: 1024,
            ..settings
        },
        SntpClient {
            interval_exponent: 11,
            ..settings
        },
        SntpClient {
            timeout_seconds: 0,
            ..settings
        },
        SntpClient {
            retries: 11,
            ..settings
        },
    ] {
        assert!(lab.preview(|c| c.shared.sntp = Some(bad)).is_err());
    }
    let p = lab.preview(|c| c.shared.sntp = Some(settings)).unwrap();
    lab.submit(&p, Delivery::Ack).unwrap();
    assert_eq!(lab.current.shared.sntp, Some(settings));
    lab.restore(&p, true).unwrap();
    assert_eq!(lab.current.shared.sntp, None);
}

#[test]
fn restoration_reconciles_its_own_lost_or_misleading_acknowledgement() {
    for (delivery, restored) in [
        (Delivery::Rejected, false),
        (Delivery::LostBeforeApply, false),
        (Delivery::AckWithoutApply, false),
        (Delivery::LostAfterApply, true),
    ] {
        let mut lab = Lab::isolated();
        let p = lab
            .preview(|c| c.ports[2].label = "Temp fixture".into())
            .unwrap();
        lab.submit(&p, Delivery::Ack).unwrap();
        assert_eq!(lab.restore_with(&p, true, delivery).is_ok(), restored);
        assert_eq!(&lab.current, if restored { p.before() } else { p.after() });
        assert_eq!(lab.submissions, 2);
    }
}
