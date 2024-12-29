import {
	Button,
	Drawer,
	DrawerBody,
	DrawerContent,
	DrawerFooter,
	DrawerHeader,
	type useDisclosure,
} from "@nextui-org/react";
import { useQuery } from "@tanstack/react-query";
import { match } from "ts-pattern";
import { commands } from "../ipc/bindings.ts";
import {
	type BrewServiceInfo,
	brewServiceInfoListSchema,
} from "../ipc/brew/brew-type.ts";

export function BrewServiceDetail({
	serviceName,
	disclosure,
}: { serviceName: string; disclosure: ReturnType<typeof useDisclosure> }) {
	const query = useQuery({
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
						{match(query.status)
							.with("pending", () => <DrawerBody>Loading...</DrawerBody>)
							.with("error", () => (
								<DrawerBody>Error: {query.error?.message}</DrawerBody>
							))
							.with("success", () => (
								<>
									<DrawerBody>
										{Boolean(query.data) && (
											<DetailRow detailInfo={query.data!} />
										)}
									</DrawerBody>
									<DrawerFooter>
										<Button color="primary" onPress={onClose}>
											Action
										</Button>
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
						<div className="truncate font-medium text-small">{`${value}`}</div>
					</div>
				);
			})}
		</>
	);
}
