import { Button, Card, CardBody, Chip, useDisclosure } from "@nextui-org/react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import { match } from "ts-pattern";
import { type BrewServiceCommand, commands } from "../ipc/bindings.ts";
import type { BrewService } from "../ipc/brew/brew-type.ts";
import { BREW_LIST_QUERY_KEY } from "../views/brew-service-list.tsx";
import { BrewServiceDetail } from "./brew-service-detail.tsx";
import { CircleChevronRightIcon } from "./icons/circle-chevron-right.tsx";

export function BrewServiceListItem({
	brewServiceListItem,
}: { brewServiceListItem: BrewService }) {
	const queryClient = useQueryClient();
	const { mutateAsync, isPending } = useMutation({
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

	const disclosure = useDisclosure();

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
		<Card className="flex-none">
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
				<div className="flex flex-nowrap items-center gap-2">
					<Button
						isLoading={isPending}
						size="sm"
						variant="faded"
						onPress={() => {
							void mutateAsync(isServiceRunning ? "Stop" : "Run");
						}}
					>
						{isServiceRunning ? "Stop" : "Run"}
					</Button>
					<Button
						isIconOnly
						aria-label="Detail"
						variant="light"
						size="sm"
						onPress={disclosure.onOpen}
					>
						<CircleChevronRightIcon size={25} color="#ccc" />
					</Button>
				</div>
			</CardBody>
			<BrewServiceDetail
				serviceName={brewServiceListItem.name}
				disclosure={disclosure}
			/>
		</Card>
	);
}
