#define _POSIX_C_SOURCE 200809L
#include "light_bridge.h"
#include <assert.h>
#include <stdio.h>
#include <time.h>
#include <string.h>
static LlbPose received;
static int has_pose;
static void callback(void *data, uint32_t event, uint32_t state, const LlbPose *pose)
{
    (void)data; (void)state;
    if (event == 2) { received = *pose; has_pose = 1; }
}
static int64_t now(void) { int64_t n; assert(llb_monotonic_time_ns(&n)==0); return n; }
static void pause_poll(void) { struct timespec t={0,1000000}; nanosleep(&t,NULL); }
int main(void)
{
    LlbHandle *h=NULL,*s=NULL;
    LlbConfig hc={sizeof(hc),1,LLB_HMD,2000,callback,NULL};
    LlbConfig sc={sizeof(sc),1,LLB_HOST,2000,NULL,NULL};
    assert(llb_create(&hc,&h)==0); assert(llb_create(&sc,&s)==0);
    LlbTimingConfig timing={.size=sizeof(timing),.abi_version=1,.tracked_device_id=17,
        .source_clock_id=22,.local_clock_id=33,.max_age_ns=100000000,.future_tolerance_ns=5000000,
        .max_uncertainty_ns=50000000,.sync_interval_ns=2000000,.request_timeout_ns=50000000,
        .mapping_validity_ns=100000000,.max_drift_ppb=200000};
    assert(llb_configure_timing(h,&timing)==0); assert(llb_configure_timing(s,&timing)==0);
    assert(llb_mock_link(h,s)==0);
    LlbEndpoint endpoint={.size=sizeof(endpoint),.abi_version=1,.family=4,.port=1234,
        .address={127,0,0,1},.host_id=7,.correlation_id=1};
    assert(llb_request_start(h,1,&endpoint)==0); assert(llb_request_start(s,1,NULL)==0);
    assert(llb_request_prepare(s)==0);
    LlbClockStatus status={.size=sizeof(status),.abi_version=1};
    int64_t end=now()+1000000000;
    for (;;) {
        assert(llb_poll(h)==0); assert(llb_poll(s)==0);
        uint32_t state,release; assert(llb_get_state(s,&state,&release)==0);
        if (state==5) assert(llb_tracking_ready(s)==0);
        assert(llb_get_clock_status(h,&status)==0);
        assert(llb_get_state(h,&state,&release)==0);
        if (state==6 && status.synchronized) break;
        assert(now()<end); pause_poll();
    }
    assert(llb_mock_clock(h,0,0,0)==2);
    LlbPose pose={.size=sizeof(pose),.abi_version=1,.session_id=1,.sequence=1,.tracked_device_id=17,
        .tracking_origin_id=8,.frame_id=9,.source_clock_id=22,.sample_timestamp_ns=now(),
        .clock_uncertainty_ns=1000,.timestamp_semantics=1,.flags=7,
        .position_m={1,2,3},.orientation_xyzw={0,0,0,1}};
    assert(llb_publish_pose(s,&pose)==0);
    while (!has_pose) { assert(llb_poll(h)==0); assert(llb_poll(s)==0); assert(now()<end); pause_poll(); }
    assert(received.session_id==pose.session_id && received.sequence==pose.sequence);
    assert(received.tracked_device_id==17 && received.tracking_origin_id==8 && received.frame_id==9);
    assert(received.source_clock_id==22 && received.sample_timestamp_ns==pose.sample_timestamp_ns);
    assert(received.timestamp_semantics==1 && received.clock_uncertainty_ns==1000 && received.flags==7);
    assert(memcmp(received.position_m,pose.position_m,sizeof(pose.position_m))==0);
    assert(memcmp(received.orientation_xyzw,pose.orientation_xyzw,sizeof(pose.orientation_xyzw))==0);
    LlbReferenceTiming rt={.size=sizeof(rt),.abi_version=1};
    assert(llb_get_reference_timing(h,&rt)==0);
    assert(rt.session_id==1 && rt.sequence==1 && rt.local_clock_id==33 && rt.mapping_generation>0);
    int64_t delta=rt.local_sample_timestamp_ns-pose.sample_timestamp_ns;
    uint64_t magnitude=delta<0?(uint64_t)(-delta):(uint64_t)delta;
    assert(magnitude<=rt.total_uncertainty_ns);
    uint64_t rejected; uint32_t reason;
    assert(llb_get_pose_diagnostics(h,&rejected,&reason)==0); assert(rejected==0 && reason==0);
    assert(llb_request_stop(s,1)==0);
    for (;;) {
        assert(llb_poll(h)==0);assert(llb_poll(s)==0);
        uint32_t hs,ss,r;assert(llb_get_state(h,&hs,&r)==0);assert(llb_get_state(s,&ss,&r)==0);
        if(hs==0 && ss==0)break;
        assert(now()<end);pause_poll();
    }
    assert(llb_get_clock_status(h,&status)==0 && !status.synchronized);
    assert(llb_get_reference_timing(h,&rt)==2);
    assert(llb_destroy(&h)==0);assert(llb_destroy(&s)==0);
    puts("Live CLOCK_MONOTONIC sync / raw pose + mapped timing / release cleanup: OK (Mock USB)");
    return 0;
}
