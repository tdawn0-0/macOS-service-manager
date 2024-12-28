import { Button, Card, CardBody, Chip } from "@nextui-org/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { match } from "ts-pattern";
import { type BrewServiceCommand, commands } from "../ipc/bindings.ts";
import {
	type BrewService,
	brewServiceListSchema,
} from "../ipc/brew/brew-type.ts";

export const BREW_LIST_QUERY_KEY = "brewServiceList" as const;

export function BrewServiceListView() {
	const query = useQuery({
		queryKey: [BREW_LIST_QUERY_KEY],
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
		mutationKey: ["manageBrewService", brewServiceListItem.name],
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
			await queryClient.cancelQueries({ queryKey: [BREW_LIST_QUERY_KEY] });

			// Snapshot the previous value
			const previousData = queryClient.getQueryData([BREW_LIST_QUERY_KEY]);

			// Optimistically update the cache
			queryClient.setQueryData([BREW_LIST_QUERY_KEY], (old: BrewService[]) => {
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

	const isServiceRunning = useMemo(() => {
		return match(brewServiceListItem.status)
			.with("started", () => true)
			.with("scheduled", () => true)
			.with("error", () => true)
			.otherwise(() => false);
	}, [brewServiceListItem.status]);

	const getSwitchColor = useMemo(() => {
		return match(brewServiceListItem.status)
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
			.otherwise(() => "default");
	}, [brewServiceListItem.status]);

	return (
		<Card>
			<CardBody className="flex flex-row flex-nowrap items-center justify-between">
				<div className="flex flex-nowrap items-center gap-2">
					<span>{brewServiceListItem.name}</span>
					<Chip size="sm" color={getSwitchColor} variant="dot">
						{brewServiceListItem.status}
					</Chip>
					{Boolean(brewServiceListItem.user) && (
						<Chip size="sm" variant="bordered">
							{brewServiceListItem.user}
						</Chip>
					)}
				</div>
				<div>
					<Button
						isLoading={isPending}
						size="sm"
						variant="faded"
						onPress={() => {
							mutate(isServiceRunning ? "Stop" : "Run");
						}}
					>
						{isServiceRunning ? "Stop" : "Run"}
					</Button>
				</div>
			</CardBody>
		</Card>
	);
}
