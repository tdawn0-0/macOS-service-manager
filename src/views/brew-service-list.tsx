import { Card, CardBody } from "@nextui-org/react";
import { useQuery } from "@tanstack/react-query";
import { commands } from "../ipc/bindings.ts";
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
	return (
		<Card>
			<CardBody className="flex flex-row flex-nowrap items-center justify-between">
				<div>{brewServiceListItem.name}</div>
				<div>Button</div>
			</CardBody>
		</Card>
	);
}
