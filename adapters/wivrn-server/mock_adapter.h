/* Runtime ordering stand-in. Real driver release acknowledgement remains Phase 2 work. */
#pragma once
#include "light_bridge.h"
static inline int32_t llb_mock_server_release_and_stop(LlbHandle *host) {
 return llb_request_stop(host,1);
}
