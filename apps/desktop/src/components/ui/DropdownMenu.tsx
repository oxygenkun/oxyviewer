import * as Primitive from "@radix-ui/react-dropdown-menu";
import type { ComponentProps } from "react";

export { Root, Trigger, Group, RadioGroup, RadioItem, CheckboxItem, Item,
  ItemIndicator, Separator, Sub, SubTrigger, Portal } from "@radix-ui/react-dropdown-menu";

/** Roles pick up the shared menu theme, including when portalled to body. */
export function Content({ container, children, onKeyDown, ...props }:
  ComponentProps<typeof Primitive.Content> & { container?: HTMLElement | null }) {
  return <Primitive.Portal container={container}>
    <Primitive.Content sideOffset={6} collisionPadding={8} loop {...props}
      onKeyDown={event => { onKeyDown?.(event); event.stopPropagation(); }}>
      {children}
    </Primitive.Content>
  </Primitive.Portal>;
}

export function SubContent({ children, onKeyDown, ...props }: ComponentProps<typeof Primitive.SubContent>) {
  return <Primitive.Portal>
    <Primitive.SubContent sideOffset={4} collisionPadding={8} loop {...props}
      onKeyDown={event => { onKeyDown?.(event); event.stopPropagation(); }}>
      {children}
    </Primitive.SubContent>
  </Primitive.Portal>;
}
