/* Local endpoint notification DTO stand-in; no WiVRn wire protocol changes. */
#pragma once
#include "light_bridge.h"
static inline LlbEndpoint llb_mock_client_endpoint(uint64_t session) {
 LlbEndpoint ep={0};
 ep.size=sizeof(ep);ep.abi_version=LLB_ABI_VERSION;ep.family=6;ep.port=4242;
 ep.address[15]=1;ep.host_id=1;ep.correlation_id=session;
 return ep;
}
