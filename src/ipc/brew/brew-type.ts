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
