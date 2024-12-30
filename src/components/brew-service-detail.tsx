import {
	Button, Divider,
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
import { commands } from "../ipc/bindings.ts";
import {
	type BrewServiceInfo,
	brewServiceInfoListSchema,
} from "../ipc/brew/brew-type.ts";
import { ActivityIcon } from "./icons/activity.tsx";
import { CopyIcon } from "./icons/copy.tsx";
import {useMutateManageBrewService} from "../hooks/use-mutate-manage-brew-service.ts";

export function BrewServiceDetail({
	serviceName,
	disclosure,
}: { serviceName: string; disclosure: ReturnType<typeof useDisclosure> }) {
	const { data, status, error } = useQuery({
		queryKey: ["getBrewServiceInfo", serviceName],
		queryFn: async (context) => {
			const res = await commands.getBrewServiceInfo(context.queryKey[1]);
			if (res.status === "error") {
				throw new Error(res.error);
			}
			const parsed = brewServiceInfoListSchema.safeParse(JSON.parse(res.data));
			if (!parsed.success) {
				throw new Error(parsed.error.message);
			}
			const data = parsed.data[0];
			if (!data) {
				throw new Error("No data");
			}
			return data;
		},
	});

	return (
		<Drawer
			isOpen={disclosure.isOpen}
			placement={"bottom"}
			onOpenChange={disclosure.onOpenChange}
		>
			<DrawerContent>
				{(onClose) => (
					<>
						<DrawerHeader className="flex flex-col">
							{serviceName}
						</DrawerHeader>
						{match([status, data])
							.with(["pending", P._], () => <DrawerBody>Loading...</DrawerBody>)
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
										<BrewServiceActions serviceName={serviceName} />
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

function DetailRow({ detailInfo }: { detailInfo: BrewServiceInfo }) {
	return (
		<>
			{Object.entries(detailInfo).map(([key, value]) => {
				return (
					<div>
						<div
							key={key}
							className="flex items-center justify-between gap-3"
						>
							<div className="text-default-500 text-small">{key}</div>
							<div className="flex min-w-0 items-center gap-1">
								<div className="truncate font-medium text-small">{`${value}`}</div>
								{key.includes("log_path") && typeof value === "string" ? (
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
											<ActivityIcon color="#ccc" size={20}/>
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
											void navigator.clipboard.writeText(`${value}`);
										}}
									>
										<CopyIcon color="#ccc" size={20}/>
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

function BrewServiceActions({serviceName}: { serviceName: string }) {
	const { mutateAsync } = useMutateManageBrewService(serviceName)

	return (
		<>
			<Tooltip content="Run without registering to launch at login">
				<Button
					size="sm"
					onPress={() => {
						void mutateAsync("Run");
					}}
				>
					Run
				</Button>
			</Tooltip>
			<Tooltip content="Start and register it to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void mutateAsync("Start");
					}}
				>
					Start
				</Button>
			</Tooltip>
			<Tooltip content="Stop and start immediately and register it to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void mutateAsync("Restart");
					}}
				>
					Restart
				</Button>
			</Tooltip>
			<Tooltip
				content="Stop immediately and unregister it from launching at
login."
			>
				<Button
					size="sm"
					onPress={() => {
						void mutateAsync("Stop");
					}}
				>
					Stop
				</Button>
			</Tooltip>
			<Tooltip content="Stop immediately but keep it registered to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void mutateAsync("Kill");
					}}
				>
					Kill
				</Button>
			</Tooltip>
		</>
	);
}
