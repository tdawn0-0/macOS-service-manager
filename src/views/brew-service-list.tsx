import {useQuery} from "@tanstack/react-query";
import {commands} from "../ipc/tauri/bindings.ts";
import {brewServiceListSchema,} from "../ipc/brew/brew-type.ts";
import {BrewServiceListItem} from "../components/brew-service-list-item.tsx";

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

	if (query.isLoading) {
		return <div>Loading...</div>;
	}

	if (!query.data) {
		return <div>No data</div>;
	}

	return (
		<div className="flex flex-col flex-nowrap gap-2 flex-grow min-h-0">
			{query.data.map((brewServiceListItem) => (
				<BrewServiceListItem
					key={brewServiceListItem.name}
					brewServiceListItem={brewServiceListItem}
				/>
			))}
		</div>
	);
}

