import { Button, Card, CardBody, Chip, useDisclosure } from "@nextui-org/react";
import { useMemo } from "react";
import { match } from "ts-pattern";
import type { BrewService } from "../ipc/brew/brew-type.ts";
import { BrewServiceDetail } from "./brew-service-detail.tsx";
import { CircleChevronRightIcon } from "./icons/circle-chevron-right.tsx";
import {useMutateManageBrewService} from "../hooks/use-mutate-manage-brew-service.ts";

export function BrewServiceListItem({
	brewServiceListItem,
}: { brewServiceListItem: BrewService }) {
	const { mutateAsync, isPending } = useMutateManageBrewService(brewServiceListItem.name)

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
