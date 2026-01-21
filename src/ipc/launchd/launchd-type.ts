import { z } from "zod";

export const launchdServiceSchema = z.object({
	label: z.string(),
	path: z.string(),
	domain: z.enum(["user", "system"]),
	loaded: z.boolean(),
	running: z.boolean(),
	pid: z.number().nullable(),
});

export type LaunchdService = z.infer<typeof launchdServiceSchema>;

export const launchdServiceListSchema = z.array(launchdServiceSchema);

export type LaunchdServiceList = z.infer<typeof launchdServiceListSchema>;

export const launchdServiceInfoSchema = z.object({
	label: z.string(),
	path: z.string(),
	domain: z.enum(["user", "system"]),
	loaded: z.boolean(),
	running: z.boolean(),
	pid: z.number().nullable(),
	program: z.string().nullable(),
	program_arguments: z.array(z.string()).nullable(),
	run_at_load: z.boolean().nullable(),
	keep_alive: z.boolean().nullable(),
	working_directory: z.string().nullable(),
	standard_out_path: z.string().nullable(),
	standard_error_path: z.string().nullable(),
});

export type LaunchdServiceInfo = z.infer<typeof launchdServiceInfoSchema>;
