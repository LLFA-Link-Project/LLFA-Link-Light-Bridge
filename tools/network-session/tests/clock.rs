//! Actual ClockSync over TCP/CLOCK_MONOTONIC, with explicit Mock USB and synthetic driver samples.
#![cfg(target_os = "linux")]
use light_bridge_core::{
    clock::{monotonic_ns, TimingConfig},
    EndpointInfo, Error, ExternalPose, State,
};
use light_bridge_hmd::HmdClient;
use light_bridge_host::HostServer;
use light_bridge_protocol::network;
use light_bridge_usbip::MockUsbIp;
use std::{
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
#[test]
fn tcp_live_sync_reference_metadata_rejections_expiry_and_new_session() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = EndpointInfo {
        addresses: vec![listener.local_addr().unwrap()],
        host_id: 7,
        correlation_id: 1,
    };
    let e = endpoint.clone();
    let server =
        thread::spawn(move || network::accept(&listener, &e, Duration::from_secs(2)).unwrap());
    let a = network::connect(&endpoint, Duration::from_secs(2)).unwrap();
    let b = server.join().unwrap();
    let mut h = HmdClient::new(
        a,
        Arc::new(Mutex::new(MockUsbIp::ready())),
        Duration::from_secs(2),
    );
    let mut s = HostServer::new(
        b,
        Arc::new(Mutex::new(MockUsbIp::remote_importer())),
        Duration::from_secs(2),
    );
    let mut c = TimingConfig::experimental(17, 22, 33);
    c.sync_interval_ns = 2_000_000;
    c.mapping_validity_ns = 100_000_000;
    c.request_timeout_ns = 50_000_000;
    c.max_uncertainty_ns = 50_000_000;
    h.configure_timing(c).unwrap();
    s.configure_timing(c).unwrap();
    s.start(1).unwrap();
    h.start(1, &endpoint).unwrap();
    s.prepare_tracking().unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        h.poll().unwrap();
        s.poll().unwrap();
        if s.session.state == State::TrackingInitializing {
            s.tracking_ready().unwrap();
        }
        if h.session.state == State::Active && h.clock_status().unwrap().synchronized == 1 {
            break;
        }
        assert!(Instant::now() < end, "clock did not converge");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        h.set_mock_clock(
            0,
            light_bridge_core::ClockMapping {
                source_clock_id: 1,
                offset_ns: 0,
                uncertainty_ns: 0
            }
        ),
        Err(Error::InvalidState)
    );
    let mut pose = ExternalPose::mock(1, 1, monotonic_ns().unwrap());
    pose.tracked_device_id = 17;
    pose.source_clock_id = 22;
    pose.timestamp_semantics = 1;
    s.publish_pose(pose).unwrap();
    let got = loop {
        h.poll().unwrap();
        s.poll().unwrap();
        if let Some(p) = h.take_reference_pose() {
            break p;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(got, pose); // Original driver timestamp/semantics stay intact.
    let timing = h.reference_timing().unwrap();
    assert_eq!(
        (timing.session_id, timing.sequence, timing.local_clock_id),
        (1, 1, 33)
    );
    assert!(
        (timing.local_sample_timestamp_ns - pose.sample_timestamp_ns).unsigned_abs()
            <= timing.total_uncertainty_ns
    );
    let mut stale = pose;
    stale.sequence = 2;
    stale.sample_timestamp_ns -= 1_000_000_000;
    s.publish_pose(stale).unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while h.pose_diagnostics().0 == 0 {
        h.poll().unwrap();
        s.poll().unwrap();
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(h.take_reference_pose().is_none());
    assert_eq!(h.pose_diagnostics().1, 2);
    thread::sleep(Duration::from_millis(120));
    assert_eq!(h.clock_status().unwrap().synchronized, 0);
    s.stop(true).unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while h.session.state != State::Idle || s.session.state != State::Idle {
        h.poll().unwrap();
        s.poll().unwrap();
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
    let mut e = endpoint;
    e.correlation_id = 2;
    s.start(2).unwrap();
    h.start(2, &e).unwrap();
    assert_eq!(h.clock_status().unwrap().sample_count, 0);
    assert!(h.reference_timing().is_err());
    s.stop(true).unwrap();
    let end = Instant::now() + Duration::from_secs(1);
    while h.session.state != State::Idle || s.session.state != State::Idle {
        h.poll().unwrap();
        s.poll().unwrap();
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(1));
    }
}
