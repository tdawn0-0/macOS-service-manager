import type React from "react";

export function Background({ children }: { children: React.ReactElement }) {
	return (
		<div className="relative isolate">
			<div className="absolute z-10 h-full w-full overflow-clip p-2 flex flex-col">
				{children}
			</div>

			<div className="relative z-0 h-screen w-full bg-dot-black/[0.2] bg-white dark:bg-black dark:bg-dot-white/[0.2]">
				<div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-white [mask-image:radial-gradient(ellipse_at_center,transparent_20%,black)] dark:bg-black" />
			</div>
		</div>
	);
}
