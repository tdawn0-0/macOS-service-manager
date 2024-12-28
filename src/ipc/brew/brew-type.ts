import { z } from "zod";

export const brewServiceSchema = z.object({
	name: z.string(),
	status: z.union([
		z.literal("started"),
		z.literal("scheduled"),
		z.literal("stopped"),
		z.literal("none"),
		z.literal("error"),
		z.literal("unknown"),
		z.literal("other"),
	]),
	user: z.union([z.string(), z.null()]),
	file: z.string(),
	exit_code: z.union([z.number(), z.null()]),
});

export type BrewService = z.infer<typeof brewServiceSchema>;

export const brewServiceListSchema = z.array(brewServiceSchema);

export type BrewServiceList = z.infer<typeof brewServiceListSchema>;

export const brewServiceInfoSchema = z.object({
	name: z.string(),
	service_name: z.string(),
	running: z.boolean(),
	loaded: z.boolean(),
	schedulable: z.boolean(),
	pid: z.union([z.number(), z.null()]),
	exit_code: z.union([z.number(), z.null()]),
	user: z.union([z.string(), z.null()]),
	status: z.string(),
	file: z.string(),
	command: z.string(),
	working_dir: z.union([z.string(), z.null()]),
	root_dir: z.union([z.string(), z.null()]),
	log_path: z.string(),
	error_log_path: z.string(),
	interval: z.union([z.number(), z.null()]),
	cron: z.union([z.string(), z.null()]),
});

export type BrewServiceInfo = z.infer<typeof brewServiceInfoSchema>;

export const brewServiceInfoListSchema = z.array(brewServiceInfoSchema);

export type BrewServiceInfoList = z.infer<typeof brewServiceInfoListSchema>;
