import {useMutation, useQueryClient} from "@tanstack/react-query";
import {BrewServiceCommand, commands} from "../ipc/bindings.ts";
import {BREW_LIST_QUERY_KEY} from "../views/brew-service-list.tsx";
import type {BrewService} from "../ipc/brew/brew-type.ts";

export function useMutateManageBrewService(serviceName: string) {
    const queryClient = useQueryClient();

    return useMutation({
        mutationKey: ["manageBrewService", serviceName],
        mutationFn: async (command: BrewServiceCommand) => {
            const res = await commands.manageBrewService(
                serviceName,
                command,
            );
            if (res.status === "error") {
                throw new Error(res.error);
            }
        },
        onMutate: async (command) => {
            // Cancel any outgoing refetches
            await queryClient.cancelQueries({ queryKey: [BREW_LIST_QUERY_KEY] });

            // Snapshot the previous value
            const previousData = queryClient.getQueryData([BREW_LIST_QUERY_KEY]);

            // Optimistically update the cache
            queryClient.setQueryData([BREW_LIST_QUERY_KEY], (old: BrewService[]) => {
                return old.map((service) => {
                    if (service.name === serviceName) {
                        return {
                            ...service,
                            status: command === "Run" ? "started" : "stopped",
                        };
                    }
                    return service;
                });
            });

            // Return context with the previous data
            return { previousData };
        },
        onError: (_err, _variables, context) => {
            // If the mutation fails, roll back to the previous value
            if (context?.previousData) {
                queryClient.setQueryData([BREW_LIST_QUERY_KEY], context.previousData);
            }
        },
        onSettled: () => {
            // Always refetch after error or success to ensure data consistency
            void queryClient.invalidateQueries({
                queryKey: [BREW_LIST_QUERY_KEY],
            });
        },
    });
}
