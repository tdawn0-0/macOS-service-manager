import { Button, Card, CardBody, Chip, useDisclosure } from "@nextui-org/react";
import type { LaunchdService } from "../ipc/tauri/bindings.ts";
import { LaunchdServiceDetail } from "./launchd-service-detail.tsx";
import { CircleChevronRightIcon } from "./icons/circle-chevron-right.tsx";
import { useMutateManageLaunchdService } from "../hooks/use-mutate-manage-launchd-service.ts";

export function LaunchdServiceListItem({
	launchdService,
}: { launchdService: LaunchdService }) {
	const { mutateAsync, isPending } = useMutateManageLaunchdService(
		launchdService.label,
		launchdService.path,
	);

	const disclosure = useDisclosure();

	const getStatusColor = () => {
		if (launchdService.running) {
			return "success";
		}
		if (launchdService.loaded) {
			return "warning";
		}
		return "default";
	};

	const getStatusText = () => {
		if (launchdService.running) {
			return "running";
		}
		if (launchdService.loaded) {
			return "loaded";
		}
		return "unloaded";
	};

	return (
		<Card className="flex-none">
			<CardBody className="flex flex-row flex-nowrap items-center justify-between">
				<div className="flex flex-nowrap items-center gap-2 min-w-0 flex-1">
					<span className="truncate">{launchdService.label}</span>
					<Chip size="sm" color={getStatusColor()} variant="dot">
						{getStatusText()}
					</Chip>
					<Chip size="sm" variant="bordered">
						{launchdService.domain}
					</Chip>
				</div>
				<div className="flex flex-nowrap items-center gap-2 flex-shrink-0">
					<Button
						isLoading={isPending}
						size="sm"
						variant="faded"
						onPress={() => {
							void mutateAsync(launchdService.loaded ? "Unload" : "Load");
						}}
					>
						{launchdService.loaded ? "Unload" : "Load"}
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
			<LaunchdServiceDetail
				launchdService={launchdService}
				disclosure={disclosure}
			/>
		</Card>
	);
}
