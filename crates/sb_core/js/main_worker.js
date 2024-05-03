import { SUPABASE_USER_WORKERS } from "ext:sb_user_workers/user_workers.js";
import { applySupabaseTag } from "ext:sb_core_main_js/js/http.js";
import { core } from "ext:core/mod.js";

const ops = core.ops;

/**
 * 
 * @param {"json" | "prometheus"} [flavor]
 * @returns {Promise<any>}
 */
function getRuntimeMetrics(flavor) {
	if (flavor !== void 0) {
		switch (flavor) {
			case "json":
			case "prometheus":
				break;
			default:
				throw new TypeError(`Invalid flavor: ${flavor}`);
		}
	} else {
		flavor = "json";
	}

	if (flavor === "json") {
		return ops.op_runtime_metrics_json();
	} else {
		return ops.op_runtime_metrics_prometheus();
	}
}

Object.defineProperty(globalThis, 'EdgeRuntime', {
	get() {
		return {
			userWorkers: SUPABASE_USER_WORKERS,
			getRuntimeMetrics: flavor => /* async */ getRuntimeMetrics(flavor),
			applySupabaseTag: (src, dest) => applySupabaseTag(src, dest),
		};
	},
	configurable: true,
});
