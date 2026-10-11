#define _POSIX_C_SOURCE 200809L
#include "light_bridge.h"
#include "../../adapters/monado-llfa/mock_adapter.h"
#include "../../adapters/wivrn-client/mock_adapter.h"
#include "../../adapters/wivrn-server/mock_adapter.h"
#include <assert.h>
#include <stddef.h>
#include <stdio.h>
#include <time.h>
#include <sys/socket.h>
#include <unistd.h>
_Static_assert(sizeof(LlbPose)==136, "pose ABI");
_Static_assert(offsetof(LlbPose,position_m)==80, "pose offset");
_Static_assert(sizeof(LlbEndpoint)==56, "endpoint ABI");
static unsigned poses, states, last_state;
static LlbMockMonado monado;
static int alive=1;
static void callback(void *userdata, uint32_t event, uint32_t state, const LlbPose *pose) {
 assert(alive && userdata == &alive);
 if(event==1) { ++states; last_state=state; }
 if(event==2) { assert(pose && pose->size==sizeof(*pose));assert(pose->session_id==42);assert(pose->position_m[0]==1);llb_mock_monado_reference(&monado,pose);++poses; }
}
int main(void) {
 LlbHandle *hmd=NULL,*host=NULL;
 LlbConfig cfg={sizeof(cfg),LLB_ABI_VERSION,LLB_HMD,1000,callback,&alive};
 assert(llb_create(NULL,&hmd)==1);
 assert(llb_create(&cfg,&hmd)==0);
 cfg.role=LLB_HOST;cfg.callback=NULL;
 assert(llb_create(&cfg,&host)==0);
 assert(llb_mock_link(hmd,host)==0);
 LlbEndpoint ep=llb_mock_client_endpoint(42);
 int sockets[2]; assert(socketpair(AF_UNIX,SOCK_STREAM,0,sockets)==0);
 LlbEndpoint notified={.size=sizeof(notified),.abi_version=1}; uint32_t kind=0;
 assert(llb_ipc_send(sockets[0],1,&ep,1000)==0);
 assert(llb_ipc_receive(sockets[1],1000,&kind,&notified)==0 && kind==1 && notified.correlation_id==42);
 assert(llb_ipc_send(sockets[0],2,&ep,1000)==0);
 assert(llb_ipc_receive(sockets[1],1000,&kind,&notified)==0 && kind==2 && notified.port==0);
 close(sockets[0]); close(sockets[1]);
 assert(llb_request_start(host,42,NULL)==0);
 assert(llb_request_prepare(host)==0);
 assert(llb_apply_notification(host,1,&ep)==2);
 assert(llb_apply_notification(hmd,1,&ep)==0);
 assert(llb_poll(hmd)==0); assert(llb_poll(host)==0);
 assert(llb_poll(hmd)==0); assert(llb_poll(host)==0); assert(llb_poll(hmd)==0);
 assert(last_state==5);
 assert(llb_tracking_ready(host)==0);assert(llb_poll(hmd)==0);assert(last_state==6);
 assert(llb_mock_clock(hmd,1000000,100,10)==0);
 LlbPose p={.size=sizeof(p),.abi_version=1,.session_id=42,.sequence=1,.tracked_device_id=1,.tracking_origin_id=1,.frame_id=1,.source_clock_id=1,.sample_timestamp_ns=999900,.timestamp_semantics=1,.flags=7,.position_m={1,2,3},.orientation_xyzw={0,0,0,1}};
 assert(llb_publish_pose(host,&p)==0);
 for(int i=0;i<100 && !poses;i++){struct timespec delay={0,1000000};nanosleep(&delay,NULL);assert(llb_poll(hmd)==0);}
 assert(poses==1);assert(states>0);assert(monado.reference_count==1 && !monado.head_pose_valid);
 LlbEndpoint wrong=ep; wrong.host_id++; assert(llb_apply_notification(hmd,2,&wrong)==3);
 assert(llb_request_stop(host,0)==9);
 assert(llb_mock_server_release_and_stop(host)==0);assert(llb_poll(hmd)==0);assert(llb_poll(host)==0);assert(last_state==0);
 unsigned before=poses;
 assert(llb_poll(hmd)==0);assert(poses==before);
 assert(llb_destroy(&host)==0);assert(host==NULL);
 assert(llb_destroy(&hmd)==0);assert(hmd==NULL);alive=0;
 assert(llb_destroy(&hmd)==0);
 /* Destroy while a pose worker exists, without start. */
 alive=1;cfg.role=LLB_HMD;cfg.callback=callback;assert(llb_create(&cfg,&hmd)==0);
 cfg.role=LLB_HOST;cfg.callback=NULL;assert(llb_create(&cfg,&host)==0);assert(llb_mock_link(hmd,host)==0);
 assert(llb_destroy(&host)==0);assert(llb_destroy(&hmd)==0);alive=0;
 puts("C ABI smoke: create/start/prepare/pose/stop/destroy passed");
}
