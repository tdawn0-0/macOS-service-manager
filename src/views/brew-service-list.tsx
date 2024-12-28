import { Card, CardBody, Switch } from "@nextui-org/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { match } from "ts-pattern";
import { type BrewServiceCommand, commands } from "../ipc/bindings.ts";
import {
	type BrewService,
	brewServiceListSchema,
} from "../ipc/brew/brew-type.ts";

export function BrewServiceListView() {
	const query = useQuery({
		queryKey: ["brewServiceList"],
		queryFn: async () => {
			const res = await commands.getBrewServices();
			if (res.status === "error") {
				throw new Error(res.error);
			}
			const parsed = brewServiceListSchema.safeParse(JSON.parse(res.data));
			if (!parsed.success) {
				throw new Error(parsed.error.message);
			}
			return parsed.data;
		},
	});
	if (query.isError) {
		return <div>Error: {query.error.message}</div>;
	}

	if (query.isFetching) {
		return <div>Loading...</div>;
	}

	if (!query.data) {
		return <div>No data</div>;
	}

	return (
		<div className="flex flex-col flex-nowrap gap-2">
			{query.data.map((brewServiceListItem) => (
				<BrewServiceListItem
					key={brewServiceListItem.name}
					brewServiceListItem={brewServiceListItem}
				/>
			))}
		</div>
	);
}

function BrewServiceListItem({
	brewServiceListItem,
}: { brewServiceListItem: BrewService }) {
	const queryClient = useQueryClient();

	const { mutate, isPending } = useMutation({
		mutationKey: [
			"manageBrewService",
			brewServiceListItem.name,
			brewServiceListItem.status,
		],
		mutationFn: async (command: BrewServiceCommand) => {
			const res = await commands.manageBrewService(
				brewServiceListItem.name,
				command,
			);
			if (res.status === "error") {
				throw new Error(res.error);
			}
		},
		onMutate: async (command) => {
			// Cancel any outgoing refetches
			await queryClient.cancelQueries({ queryKey: ["brewServiceList"] });

			// Snapshot the previous value
			const previousData = queryClient.getQueryData(["brewServiceList"]);

			// Optimistically update the cache
			queryClient.setQueryData(["brewServiceList"], (old: BrewService[]) => {
				return old.map((service) => {
					if (service.name === brewServiceListItem.name) {
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
				queryClient.setQueryData(["brewServiceList"], context.previousData);
			}
		},
		onSettled: () => {
			// Always refetch after error or success to ensure data consistency
			void queryClient.invalidateQueries({
				queryKey: ["brewServiceList"],
			});
		},
	});

	const isSwitchOn = match(brewServiceListItem.status)
		.with("started", () => true)
		.with("scheduled", () => true)
		.with("error", () => true)
		.otherwise(() => false);

	return (
		<Card>
			<CardBody className="flex flex-row flex-nowrap items-center justify-between">
				<div>{brewServiceListItem.name}</div>
				<div>
					<Switch
						size="sm"
						isDisabled={isPending}
						isSelected={isSwitchOn}
						aria-label={`Turn on/off ${brewServiceListItem.name}`}
						color={match(brewServiceListItem.status)
							.returnType<
								| "default"
								| "success"
								| "warning"
								| "primary"
								| "secondary"
								| "danger"
								| undefined
							>()
							.with("started", () => "success")
							.with("scheduled", () => "warning")
							.with("error", () => "danger")
							.otherwise(() => "default")}
						onValueChange={() => {
							mutate(isSwitchOn ? "Stop" : "Run");
						}}
					/>
				</div>
			</CardBody>
		</Card>
	);
}
