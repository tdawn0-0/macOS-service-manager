import {
	Button,
	Divider,
	Drawer,
	DrawerBody,
	DrawerContent,
	DrawerFooter,
	DrawerHeader,
	Tooltip,
	type useDisclosure,
} from "@nextui-org/react";
import { useQuery } from "@tanstack/react-query";
import { P, match } from "ts-pattern";
import { commands, type LaunchdService } from "../ipc/tauri/bindings.ts";
import { CopyIcon } from "./icons/copy.tsx";
import { ActivityIcon } from "./icons/activity.tsx";
import { useMutateManageLaunchdService } from "../hooks/use-mutate-manage-launchd-service.ts";

export function LaunchdServiceDetail({
	launchdService,
	disclosure,
}: {
	launchdService: LaunchdService;
	disclosure: ReturnType<typeof useDisclosure>;
}) {
	const { data, status, error } = useQuery({
		queryKey: ["getLaunchdServiceInfo", launchdService.path],
		queryFn: async (context) => {
			const res = await commands.getLaunchdServiceInfo(context.queryKey[1]);
			if (res.status === "error") {
				throw new Error(res.error);
			}
			return res.data;
		},
		enabled: disclosure.isOpen,
	});

	return (
		<Drawer
			isOpen={disclosure.isOpen}
			placement={"bottom"}
			onOpenChange={disclosure.onOpenChange}
		>
			<DrawerContent>
				{() => (
					<>
						<DrawerHeader className="flex flex-col">
							{launchdService.label}
						</DrawerHeader>
						{match([status, data])
							.with(["pending", P._], () => (
								<DrawerBody>Loading...</DrawerBody>
							))
							.with(["error", P._], () => (
								<DrawerBody>Error: {error?.message}</DrawerBody>
							))
							.with(["success", P.not(P.nullish)], () => (
								<>
									<DrawerBody className="gap-2">
										{/* biome-ignore lint/style/noNonNullAssertion: <explanation> */}
										<DetailRow detailInfo={data!} />
									</DrawerBody>
									<DrawerFooter className="justify-between">
										<LaunchdServiceActions
											label={launchdService.label}
											path={launchdService.path}
											loaded={launchdService.loaded}
											running={launchdService.running}
										/>
									</DrawerFooter>
								</>
							))
							.otherwise(() => (
								<DrawerBody>No data</DrawerBody>
							))}
					</>
				)}
			</DrawerContent>
		</Drawer>
	);
}

type DetailInfo = {
	label: string;
	path: string;
	domain: string;
	loaded: boolean;
	running: boolean;
	pid: number | null;
	program: string | null;
	program_arguments: string[] | null;
	run_at_load: boolean | null;
	keep_alive: boolean | null;
	working_directory: string | null;
	standard_out_path: string | null;
	standard_error_path: string | null;
};

function DetailRow({ detailInfo }: { detailInfo: DetailInfo }) {
	const displayEntries = [
		["label", detailInfo.label],
		["path", detailInfo.path],
		["domain", detailInfo.domain],
		["loaded", detailInfo.loaded],
		["running", detailInfo.running],
		["pid", detailInfo.pid],
		["program", detailInfo.program],
		[
			"program_arguments",
			detailInfo.program_arguments?.join(" ") ?? null,
		],
		["run_at_load", detailInfo.run_at_load],
		["keep_alive", detailInfo.keep_alive],
		["working_directory", detailInfo.working_directory],
		["standard_out_path", detailInfo.standard_out_path],
		["standard_error_path", detailInfo.standard_error_path],
	].filter(([_, value]) => value !== null && value !== undefined);

	return (
		<>
			{displayEntries.map(([key, value]) => {
				const keyStr = key as string;
				const valueStr = `${value}`;
				const isLogPath =
					keyStr === "standard_out_path" || keyStr === "standard_error_path";

				return (
					<div key={keyStr}>
						<div className="flex items-center justify-between gap-3">
							<div className="text-default-500 text-small">{keyStr}</div>
							<div className="flex min-w-0 items-center gap-1">
								<div className="truncate font-medium text-small">
									{valueStr}
								</div>
								{isLogPath && typeof value === "string" ? (
									<Tooltip size="sm" content="Open log file">
										<Button
											isIconOnly
											aria-label="Open log"
											size="sm"
											variant="light"
											onPress={() => {
												void commands.openLogInConsole(value);
											}}
										>
											<ActivityIcon color="#ccc" size={20} />
										</Button>
									</Tooltip>
								) : null}
								<Tooltip size="sm" content="Copy">
									<Button
										isIconOnly
										aria-label="copy"
										size="sm"
										variant="light"
										onPress={() => {
											void navigator.clipboard.writeText(valueStr);
										}}
									>
										<CopyIcon color="#ccc" size={20} />
									</Button>
								</Tooltip>
							</div>
						</div>
						<Divider className="bg-default-100 mt-2" />
					</div>
				);
			})}
		</>
	);
}

function LaunchdServiceActions({
	label,
	path,
	loaded,
	running,
}: { label: string; path: string; loaded: boolean; running: boolean }) {
	const { mutateAsync } = useMutateManageLaunchdService(label, path);

	return (
		<>
			{!loaded && (
				<Tooltip content="Load service (register with launchd)">
					<Button
						size="sm"
						onPress={() => {
							void mutateAsync("Load");
						}}
					>
						Load
					</Button>
				</Tooltip>
			)}
			{loaded && (
				<Tooltip content="Unload service (unregister from launchd)">
					<Button
						size="sm"
						onPress={() => {
							void mutateAsync("Unload");
						}}
					>
						Unload
					</Button>
				</Tooltip>
			)}
			{loaded && !running && (
				<Tooltip content="Start the service">
					<Button
						size="sm"
						onPress={() => {
							void mutateAsync("Start");
						}}
					>
						Start
					</Button>
				</Tooltip>
			)}
			{loaded && running && (
				<Tooltip content="Stop the running service">
					<Button
						size="sm"
						onPress={() => {
							void mutateAsync("Stop");
						}}
					>
						Stop
					</Button>
				</Tooltip>
			)}
		</>
	);
}
