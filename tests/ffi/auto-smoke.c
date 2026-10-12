#define _POSIX_C_SOURCE 200809L
#include "light_bridge.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/socket.h>
#include <netinet/in.h>
int main(void) {
    int listener=socket(AF_INET,SOCK_STREAM,0);assert(listener>=0);
    struct sockaddr_in address={.sin_family=AF_INET,.sin_addr.s_addr=htonl(INADDR_LOOPBACK)};
    assert(bind(listener,(struct sockaddr *)&address,sizeof(address))==0);assert(listen(listener,1)==0);
    socklen_t length=sizeof(address);assert(getsockname(listener,(struct sockaddr *)&address,&length)==0);
    int client=socket(AF_INET,SOCK_STREAM,0);assert(connect(client,(struct sockaddr *)&address,sizeof(address))==0);
    int host=accept(listener,NULL,NULL);assert(host>=0);close(listener);
    int flags=fcntl(host,F_GETFL);uint8_t request[8192],reply[8192],command[8192];uint32_t n=0,m=0,c=0;
    assert(llb_bootstrap_new_request(host,NULL,request,sizeof(request),&n)==0);assert(n>0);
    assert(llb_bootstrap_new_request(-1,NULL,reply,sizeof(reply),&m)==1);
    LlbBootstrapHandle *bootstrap=NULL;assert(llb_bootstrap_create(request,n,&bootstrap)==0);
    assert(llb_bootstrap_commit(bootstrap,reply,sizeof(reply),&m)==2);
    assert(llb_bootstrap_helper_command(request,n,1,NULL,0,0,LLB_HMD,command,sizeof(command),&c)==0);assert(c>0);
    int uds[2];assert(socketpair(AF_UNIX,SOCK_STREAM,0,uds)==0);
    assert(llb_bootstrap_ipc_send(uds[0],request,n,host,1000)==0);int route=-1;
    assert(llb_bootstrap_ipc_receive(uds[1],&route,reply,sizeof(reply),&m,1000)==0);assert(route>=0);assert(n==m&&memcmp(request,reply,n)==0);
    assert(fcntl(host,F_GETFL)==flags);assert(fcntl(route,F_GETFD)&FD_CLOEXEC);close(route);close(uds[0]);close(uds[1]);
    LlbAutoHandle *worker=NULL;assert(llb_auto_worker_create(LLB_HMD,&worker)==0);uint64_t operation=42;
    assert(llb_auto_worker_poll(worker,&operation,reply,sizeof(reply),&m)==6);assert(operation==0&&m==0);
    assert(llb_auto_worker_submit(worker,1,(const uint8_t *)"[]",2,-1)==1);assert(llb_auto_worker_destroy(&worker)==0&&worker==NULL);
    LlbHandle *runtime=NULL;LlbConfig config={.size=sizeof(config),.abi_version=1,.role=LLB_HOST,.timeout_ms=1000};assert(llb_create(&config,&runtime)==0);
    assert(llb_bootstrap_require(runtime)==0);assert(llb_tcp_listen(runtime,NULL)==2);assert(llb_tcp_listen_mock(runtime,NULL)==2);assert(llb_destroy(&runtime)==0);
    assert(llb_bootstrap_destroy(&bootstrap)==0&&bootstrap==NULL);assert(llb_bootstrap_destroy(&bootstrap)==0);
    assert(write(client,"pose",4)==4);char data[4];assert(read(host,data,4)==4&&memcmp(data,"pose",4)==0);close(client);close(host);
    puts("Automatic bootstrap C ABI: connected route, fresh IDs, duplex TCP-FD transfer, commit gate, worker and no Mock fallback passed");
}
