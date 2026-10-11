#define _POSIX_C_SOURCE 200809L
#include "light_bridge.h"
#include "../../adapters/monado-llfa/mock_adapter.h"
#include "../../adapters/wivrn-server/mock_adapter.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <time.h>
static LlbMockMonado monado;
static void callback(void *data, uint32_t event, uint32_t state, const LlbPose *pose) {
 (void)data; (void)state;
 if(event==2) llb_mock_monado_reference(&monado,pose);
}
static void *accept_host(void *host) {
 assert(llb_tcp_accept_mock(host)==0); return NULL;
}
static void wait_states(LlbHandle *hmd,LlbHandle *host,uint32_t state) {
 for(unsigned i=0;i<2000;i++) {
  assert(llb_poll(hmd)==0); assert(llb_poll(host)==0);
  uint32_t a,b,release; assert(llb_get_state(hmd,&a,&release)==0); assert(llb_get_state(host,&b,&release)==0);
  if(a==state && b==state) return;
  struct timespec delay={0,1000000}; nanosleep(&delay,NULL);
 }
 assert(0 && "state deadline");
}
int main(void) {
 LlbHandle *hmd=NULL,*host=NULL;
 LlbConfig cfg={sizeof(cfg),1,LLB_HMD,3000,callback,NULL};
 assert(llb_create(&cfg,&hmd)==0); cfg.role=LLB_HOST; cfg.callback=NULL;
 assert(llb_create(&cfg,&host)==0);
 LlbEndpoint ep={.size=sizeof(ep),.abi_version=1,.family=4,.address={127,0,0,1},.host_id=17,.correlation_id=42};
 assert(llb_tcp_listen_mock(host,&ep)==0 && ep.port!=0);
 assert(llb_mock_link(hmd,host)==2);
 pthread_t thread; assert(pthread_create(&thread,NULL,accept_host,host)==0);
 assert(llb_tcp_connect_mock(hmd,&ep)==0); assert(pthread_join(thread,NULL)==0);
 assert(llb_request_start(host,42,NULL)==0); assert(llb_request_prepare(host)==0);
 assert(llb_request_start(hmd,42,&ep)==0); wait_states(hmd,host,5);
 assert(llb_tracking_ready(host)==0); wait_states(hmd,host,6);
 assert(llb_mock_clock(hmd,1000000,100,10)==0);
 LlbPose pose={.size=sizeof(pose),.abi_version=1,.session_id=42,.sequence=1,.tracked_device_id=1,.tracking_origin_id=1,.frame_id=1,.source_clock_id=1,.sample_timestamp_ns=999900,.timestamp_semantics=1,.flags=7,.position_m={1,2,3},.orientation_xyzw={0,0,0,1}};
 assert(llb_publish_pose(host,&pose)==0);
 for(int i=0;i<1000 && !monado.reference_count;i++) {
  struct timespec delay={0,1000000}; nanosleep(&delay,NULL); assert(llb_poll(hmd)==0);
 }
 assert(monado.reference_count==1 && !monado.head_pose_valid);
 assert(llb_request_stop(host,0)==9);
 assert(llb_mock_server_release_and_stop(host)==0); wait_states(hmd,host,0);
 assert(llb_destroy(&host)==0); assert(llb_destroy(&hmd)==0);
 puts("TCP C ABI / independent Mock USB / Monado reference: OK"); return 0;
}
