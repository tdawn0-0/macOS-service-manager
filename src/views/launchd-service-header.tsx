import { Button } from "@nextui-org/react";
import { useQueryClient } from "@tanstack/react-query";
import { RotateCw } from "lucide-react";
import { LAUNCHD_LIST_QUERY_KEY } from "./launchd-service-list.tsx";

export function LaunchdServiceHeader() {
	const queryClient = useQueryClient();

	const handleRefetch = () => {
		void queryClient.invalidateQueries({
			queryKey: [LAUNCHD_LIST_QUERY_KEY],
		});
	};

	return (
		<div className="mt-1 mb-4 flex flex-nowrap items-center justify-between flex-none">
			<p className="relative bg-gradient-to-b from-neutral-200 to-neutral-500 bg-clip-text font-bold text-2xl text-transparent">
				LaunchAgents
			</p>
			<div>
				<Button
					isIconOnly
					aria-label="refresh"
					size="sm"
					variant="light"
					onPress={handleRefetch}
				>
					<RotateCw color="#ccc" />
				</Button>
			</div>
		</div>
	);
}
