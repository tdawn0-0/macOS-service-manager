import {
	Button,
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
import { CopyIcon } from "./icons/copy.tsx";

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
						<DrawerHeader className="flex flex-col gap-1">
							{serviceName}
						</DrawerHeader>
						{match([status, data])
							.with(["pending", P._], () => <DrawerBody>Loading...</DrawerBody>)
							.with(["error", P._], () => (
								<DrawerBody>Error: {error?.message}</DrawerBody>
							))
							.with(["success", P.not(P.nullish)], () => (
								<>
									<DrawerBody>
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
					<div
						key={key}
						className="flex items-center justify-between gap-3 py-2"
					>
						<div className="text-default-500 text-small">{key}</div>
						<div className="flex min-w-0 items-center gap-1">
							<div className="truncate font-medium text-small">{`${value}`}</div>
							<Button
								isIconOnly
								aria-label="refresh"
								size="sm"
								variant="light"
								onPress={() => {
									void navigator.clipboard.writeText(`${value}`);
								}}
							>
								<CopyIcon color="#ccc" />
							</Button>
						</div>
					</div>
				);
			})}
		</>
	);
}

function BrewServiceActions({ serviceName }: { serviceName: string }) {
	return (
		<>
			<Tooltip content="Run without registering to launch at login">
				<Button
					size="sm"
					onPress={() => {
						void commands.manageBrewService(serviceName, "Run");
					}}
				>
					Run
				</Button>
			</Tooltip>
			<Tooltip content="Start and register it to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void commands.manageBrewService(serviceName, "Start");
					}}
				>
					Start
				</Button>
			</Tooltip>
			<Tooltip content="Stop and start immediately and register it to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void commands.manageBrewService(serviceName, "Restart");
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
						void commands.manageBrewService(serviceName, "Stop");
					}}
				>
					Stop
				</Button>
			</Tooltip>
			<Tooltip content="Stop immediately but keep it registered to launch at login.">
				<Button
					size="sm"
					onPress={() => {
						void commands.manageBrewService(serviceName, "Kill");
					}}
				>
					Kill
				</Button>
			</Tooltip>
		</>
	);
}
