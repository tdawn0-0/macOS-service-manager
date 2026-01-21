import { useMutation, useQueryClient } from "@tanstack/react-query";
import { type LaunchdServiceCommand, commands } from "../ipc/tauri/bindings.ts";
import { LAUNCHD_LIST_QUERY_KEY } from "../views/launchd-service-list.tsx";
import type { LaunchdService } from "../ipc/launchd/launchd-type.ts";

export function useMutateManageLaunchdService(label: string, path: string) {
	const queryClient = useQueryClient();

	return useMutation({
		mutationKey: ["manageLaunchdService", label],
		mutationFn: async (command: LaunchdServiceCommand) => {
			const res = await commands.manageLaunchdService(label, path, command);
			if (res.status === "error") {
				throw new Error(res.error);
			}
		},
		onMutate: async (command) => {
			// Cancel any outgoing refetches
			await queryClient.cancelQueries({ queryKey: [LAUNCHD_LIST_QUERY_KEY] });

			// Snapshot the previous value
			const previousData = queryClient.getQueryData([LAUNCHD_LIST_QUERY_KEY]);

			// Optimistically update the cache
			queryClient.setQueryData(
				[LAUNCHD_LIST_QUERY_KEY],
				(old: LaunchdService[] | undefined) => {
					if (!old) return old;
					return old.map((service) => {
						if (service.label === label) {
							if (command === "Load") {
								return { ...service, loaded: true };
							}
							if (command === "Unload") {
								return { ...service, loaded: false, running: false, pid: null };
							}
							if (command === "Start") {
								return { ...service, running: true };
							}
							if (command === "Stop") {
								return { ...service, running: false, pid: null };
							}
						}
						return service;
					});
				},
			);

			// Return context with the previous data
			return { previousData };
		},
		onError: (_err, _variables, context) => {
			// If the mutation fails, roll back to the previous value
			if (context?.previousData) {
				queryClient.setQueryData(
					[LAUNCHD_LIST_QUERY_KEY],
					context.previousData,
				);
			}
		},
		onSettled: () => {
			// Always refetch after error or success to ensure data consistency
			void queryClient.invalidateQueries({
				queryKey: [LAUNCHD_LIST_QUERY_KEY],
			});
		},
	});
}
