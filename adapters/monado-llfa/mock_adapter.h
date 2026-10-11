/* Phase 1 stand-in for a Monado LLFA external-reference sink. No xrt API invented. */
#pragma once
#include "light_bridge.h"
#include <assert.h>
typedef struct {
 uint32_t reference_count;
 uint32_t head_pose_valid;
 LlbPose latest_reference;
} LlbMockMonado;
static inline void llb_mock_monado_reference(LlbMockMonado *sink, const LlbPose *p) {
 assert(sink && p);
 sink->latest_reference=*p;
 ++sink->reference_count;
 /* No T_HMD_Tracker calibration or fusion: cannot claim a valid head pose. */
 sink->head_pose_valid=0;
}
