// SPDX-License-Identifier: AGPL-3.0-only
// Lays a large map out off the main thread, so the window stays usable while
// a first open or a re-balance takes a few seconds. It sends back every
// body's spot (and R2 region); the map then places those spots the way it
// places a kept layout.
import { runAtlasLayoutJob, type AtlasLayoutReply, type AtlasLayoutRequest } from "./atlasLayoutJob";

export type { AtlasLayoutReply, AtlasLayoutRequest };

self.onmessage = (event: MessageEvent<AtlasLayoutRequest>) => {
  (self as unknown as Worker).postMessage(runAtlasLayoutJob(event.data) satisfies AtlasLayoutReply);
};
